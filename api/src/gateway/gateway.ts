import {type Classification, classifyRequest} from "./classify"
import type {FieldMode, GatewayConfig} from "./config"

/**
 * The GraphQL gateway: a strangler in front of PostGraphile.
 *
 * Each operation is classified by its root fields and then answered. Today every field is delegated, so the
 * original request goes to `delegate` untouched. The per-field resolvers that will serve a field from
 * geo-db (flag `shadow`, then `on`) plug in through `GatewayDeps.resolvers`; none exist yet.
 */

export type Delegate<Ctx> = (request: Request, context: Ctx) => Promise<Response> | Response

/** Where an operation is answered. "delegate" is the only route until a resolver is registered. */
export type Route = {kind: "delegate"; reason: "gateway_off" | "unclassified" | "no_resolver" | "field_off"}

export type GatewayDeps<Ctx> = {
	config: GatewayConfig
	delegate: Delegate<Ctx>
	/** Root fields with a geo-db resolver. Empty until a field is migrated. */
	resolvers?: ReadonlySet<string>
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

export function createGateway<Ctx>(deps: GatewayDeps<Ctx>): Delegate<Ctx> {
	const resolvers = deps.resolvers ?? new Set<string>()
	return async (request, context) => {
		if (deps.config.mode === "off") return deps.delegate(request, context)
		const classification = await classifyRequest(request)
		const route = routeOperation(deps.config, resolvers, classification)
		try {
			deps.onClassified?.(classification, route)
		} catch {
			// Observability must not fail a read.
		}
		return deps.delegate(request, context)
	}
}
