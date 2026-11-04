import type {DocumentNode} from "graphql"
import {type Classification, classifyRequestWithDocument, type GraphqlRequestParams} from "./classify"
import type {FieldMode, GatewayConfig} from "./config"

/**
 * The GraphQL gateway: a strangler in front of PostGraphile.
 *
 * Each operation is classified by its root fields and then answered. Today every field is delegated, so the
 * original request goes to `delegate` untouched. The per-field resolvers that will serve a field from
 * geo-db (flag `shadow`, then `on`) plug in through `GatewayDeps.resolvers`; none exist yet.
 */

/** What a field's shadow gets: the request as parsed and PostGraphile's answer (a clone, safe to read). */
export type ShadowContext = {
	classification: Classification
	params: GraphqlRequestParams
	document: DocumentNode
	response: Response
}

/** Compares geo-db's answer with PostGraphile's for one field. Must settle without throwing; the gateway ignores it. */
export type FieldShadow = (context: ShadowContext) => Promise<void>

/** PostGraphile responses larger than this are not shadowed, so a clone never holds a huge body. */
export const MAX_SHADOW_RESPONSE_BYTES = 1_000_000

export type Delegate<Ctx> = (request: Request, context: Ctx) => Promise<Response> | Response

/** Where an operation is answered. "delegate" is the only route until a resolver is registered. */
export type Route = {kind: "delegate"; reason: "gateway_off" | "unclassified" | "no_resolver" | "field_off"}

export type GatewayDeps<Ctx> = {
	config: GatewayConfig
	delegate: Delegate<Ctx>
	/** Root fields with a geo-db resolver. Empty until a field is migrated. */
	resolvers?: ReadonlySet<string>
	/** Root fields with a shadow comparison, run when the field's mode is "shadow". */
	shadows?: ReadonlyMap<string, FieldShadow>
	/** Sampling draw in [0, 1); injectable for tests. */
	random?: () => number
	onClassified?: (classification: Classification, route: Route) => void
}

export function fieldModeOf(config: GatewayConfig, field: string): FieldMode {
	return config.fieldModes.get(field) ?? "off"
}

/** Decides where an operation goes. Delegates unless every root field has a resolver and a mode other than off. */
export function routeOperation(
	config: GatewayConfig,
	resolvers: ReadonlySet<string>,
	classification: Classification,
): Route {
	if (config.mode === "off") return {kind: "delegate", reason: "gateway_off"}
	if (classification.operationType !== "query" || classification.rootFields.length === 0) {
		return {kind: "delegate", reason: "unclassified"}
	}
	if (!classification.rootFields.every((field) => resolvers.has(field)))
		return {kind: "delegate", reason: "no_resolver"}
	if (classification.rootFields.every((field) => fieldModeOf(config, field) === "off")) {
		return {kind: "delegate", reason: "field_off"}
	}
	// Unreachable until a resolver is registered; routing to one belongs to the PR that adds the first.
	return {kind: "delegate", reason: "no_resolver"}
}

/**
 * Starts the shadow comparison for an operation whose root fields are all one shadowed field, for a sampled
 * fraction of them. Everything here is synchronous and cheap (a clone of the response); the comparison runs on
 * a later tick and its failures are swallowed, so the user's response is never delayed or failed by it.
 */
function scheduleShadow<Ctx>(
	deps: GatewayDeps<Ctx>,
	classified: {classification: Classification; params: GraphqlRequestParams | null; document: DocumentNode | null},
	response: Response,
): void {
	try {
		const {classification, params, document} = classified
		const shadows = deps.shadows
		if (!shadows || !params || !document) return
		if (classification.operationType !== "query" || classification.rootFields.length !== 1) return
		const field = classification.rootFields[0] as string
		const shadow = shadows.get(field)
		if (!shadow || fieldModeOf(deps.config, field) !== "shadow") return
		if (!((deps.random ?? Math.random)() < deps.config.shadowSampleRate)) return
		if (!response.ok || !(response.headers.get("content-type") ?? "").includes("json")) return
		if (Number(response.headers.get("content-length") ?? 0) > MAX_SHADOW_RESPONSE_BYTES) return
		const clone = response.clone()
		setTimeout(() => {
			shadow({classification, params, document, response: clone}).catch(() => {})
		}, 0)
	} catch {
		// A shadow must never fail a read.
	}
}

export function createGateway<Ctx>(deps: GatewayDeps<Ctx>): Delegate<Ctx> {
	const resolvers = deps.resolvers ?? new Set<string>()
	return async (request, context) => {
		if (deps.config.mode === "off") return deps.delegate(request, context)
		const classified = await classifyRequestWithDocument(request)
		const route = routeOperation(deps.config, resolvers, classified.classification)
		try {
			deps.onClassified?.(classified.classification, route)
		} catch {
			// Observability must not fail a read.
		}
		const response = await deps.delegate(request, context)
		scheduleShadow(deps, classified, response)
		return response
	}
}
