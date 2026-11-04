import type {DocumentNode} from "graphql"
import {type Classification, classifyRequestWithDocument, type GraphqlRequestParams} from "./classify"
import type {FieldMode, GatewayConfig} from "./config"
import {activateServeMetrics, observeServe, recordServeFallback, recordServeSource} from "./serveMetrics"

/**
 * The GraphQL gateway: a strangler in front of PostGraphile.
 *
 * Each operation is classified by its root fields and then answered. An operation whose only root field is in
 * mode "on" and has a resolver in `GatewayDeps.resolvers` is offered to that resolver first, which answers it
 * from geo-db or declines; everything else, and everything a resolver declines, fails or does not answer within
 * `serveTimeoutMs`, goes to `delegate` (PostGraphile) untouched.
 */

/** What a field's shadow gets: the request as parsed and PostGraphile's answer (a clone, safe to read). */
export type ShadowContext = {
	classification: Classification
	params: GraphqlRequestParams
	document: DocumentNode
	response: Response
}

/** Compares geo-db's answer with PostGraphile's for one field. Must settle without throwing; the gateway ignores it. */
export type FieldShadow = ((context: ShadowContext) => Promise<void>) & {
	/** Other root fields the shadow tolerates beside its own (it records them as uncompared). */
	tolerates?: readonly string[]
}

/** PostGraphile responses larger than this are not shadowed, so a clone never holds a huge body. */
export const MAX_SHADOW_RESPONSE_BYTES = 1_000_000

export type Delegate<Ctx> = (request: Request, context: Ctx) => Promise<Response> | Response

/** What a field's resolver gets: the request as parsed, and a signal that aborts when its time budget runs out. */
export type ResolveContext = {
	classification: Classification
	params: GraphqlRequestParams
	document: DocumentNode
	signal: AbortSignal
}

/**
 * geo-db's answer for one root field, or the reason it declined. `data` is the whole GraphQL `data` object, keyed
 * by response key, exactly as PostGraphile would have returned it. A resolver answers only when the answer is
 * exact; anything it cannot answer exactly it declines, and PostGraphile answers instead. It may also throw (a
 * thrown error with a string `reason`, such as a GeoDbError, is counted under that reason).
 */
export type ResolveResult = {ok: true; data: Record<string, unknown>} | {ok: false; reason: string}

export type FieldResolver = (context: ResolveContext) => Promise<ResolveResult>

/** Where an operation is answered. */
export type Route =
	| {kind: "serve"; field: string; reason: "field_on"}
	| {
			kind: "delegate"
			reason: "gateway_off" | "unclassified" | "no_resolver" | "field_off" | "not_served"
	  }

export type GatewayDeps<Ctx> = {
	config: GatewayConfig
	delegate: Delegate<Ctx>
	/** Root fields with a geo-db resolver, used when the field's mode is "on". */
	resolvers?: ReadonlyMap<string, FieldResolver>
	/** Root fields with a shadow comparison, run when the field's mode is "shadow". */
	shadows?: ReadonlyMap<string, FieldShadow>
	/** Sampling draw in [0, 1); injectable for tests. */
	random?: () => number
	onClassified?: (classification: Classification, route: Route) => void
}

export function fieldModeOf(config: GatewayConfig, field: string): FieldMode {
	return config.fieldModes.get(field) ?? "off"
}

/**
 * Decides where an operation goes. It is served only when its single root field has a resolver and mode "on";
 * an operation with several root fields is delegated whole (a partial answer from two stores is not served).
 */
export function routeOperation(
	config: GatewayConfig,
	resolvers: {has(field: string): boolean},
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
	const [field] = classification.rootFields
	if (classification.rootFields.length === 1 && field !== undefined && fieldModeOf(config, field) === "on") {
		return {kind: "serve", field, reason: "field_on"}
	}
	return {kind: "delegate", reason: "not_served"}
}

const TIMED_OUT: unique symbol = Symbol("timeout")

/** Runs a resolver within the budget. Never throws: a decline, a throw and a timeout all come back as a reason. */
async function runResolver(resolver: FieldResolver, context: Omit<ResolveContext, "signal">, budgetMs: number) {
	const controller = new AbortController()
	let timer: ReturnType<typeof setTimeout> | undefined
	const timeout = new Promise<typeof TIMED_OUT>((resolve) => {
		timer = setTimeout(() => resolve(TIMED_OUT), budgetMs)
	})
	try {
		const outcome = await Promise.race([resolver({...context, signal: controller.signal}), timeout])
		if (outcome === TIMED_OUT) {
			controller.abort()
			return {ok: false as const, reason: "timeout"}
		}
		return outcome
	} catch (error) {
		const reason = (error as {reason?: unknown} | null)?.reason
		return {ok: false as const, reason: typeof reason === "string" ? reason : "error"}
	} finally {
		clearTimeout(timer)
	}
}

/** The served answer, shaped like PostGraphile's: `{"data": ...}`, JSON, status 200. */
function servedResponse(data: Record<string, unknown>): Response {
	return new Response(JSON.stringify({data}), {
		status: 200,
		headers: {"content-type": "application/json; charset=utf-8", "x-gaia-source": "geodb"},
	})
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
		if (classification.operationType !== "query" || classification.rootFields.length === 0) return
		// The shadowed field is the first root field with a shadow; every other root field must be one it tolerates.
		const field = classification.rootFields.find((name) => shadows.has(name))
		const shadow = field === undefined ? undefined : shadows.get(field)
		if (field === undefined || !shadow || fieldModeOf(deps.config, field) !== "shadow") return
		const tolerated = shadow.tolerates ?? []
		if (!classification.rootFields.every((name) => name === field || tolerated.includes(name))) return
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
	const resolvers: ReadonlyMap<string, FieldResolver> = deps.resolvers ?? new Map()
	if (deps.config.mode === "on")
		for (const field of resolvers.keys()) if (fieldModeOf(deps.config, field) === "on") activateServeMetrics(field)
	return async (request, context) => {
		if (deps.config.mode === "off") return deps.delegate(request, context)
		const classified = await classifyRequestWithDocument(request)
		const route = routeOperation(deps.config, resolvers, classified.classification)
		try {
			deps.onClassified?.(classified.classification, route)
		} catch {
			// Observability must not fail a read.
		}
		const resolver = route.kind === "serve" ? resolvers.get(route.field) : undefined
		if (route.kind === "serve" && resolver && classified.params && classified.document) {
			const started = performance.now()
			const result = await runResolver(
				resolver,
				{classification: classified.classification, params: classified.params, document: classified.document},
				deps.config.serveTimeoutMs,
			)
			const seconds = (performance.now() - started) / 1000
			if (result.ok) {
				observeServe(route.field, seconds, "served")
				recordServeSource(route.field, "geodb")
				return servedResponse(result.data)
			}
			observeServe(route.field, seconds, "fallback")
			recordServeFallback(route.field, result.reason)
			recordServeSource(route.field, "postgres")
			return deps.delegate(request, context)
		}
		const response = await deps.delegate(request, context)
		scheduleShadow(deps, classified, response)
		return response
	}
}
