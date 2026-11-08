/**
 * The `entitiesConnection` shadow: the connection form of `entities` (`entitiesConnection(first, after, filter,
 * orderBy, spaceIds, typeIds) { nodes { <light entity selection> } totalCount pageInfo }`).
 *
 * PostGraphile answers; for a sampled share of operations geo-db is asked about a few of the returned `nodes`, each
 * judged on its own card exactly as the light `entities` shadow judges them (lightShadow.ts). What the page holds
 * and its order stay uncompared (`filter_completeness`, `order_completeness`), `totalCount` is `total_count`, and
 * `pageInfo` is not read. A page after a cursor (`after`) is never placed in the order. `edges { node { .. } cursor }`
 * is read as the same nodes. An operation that asks only `totalCount` (one root, or several aliased roots) holds no
 * node to judge and is counted `unsupported_selection{count_only}`.
 *
 * It only runs when GRAPHQL_GATEWAY_FIELDS names `entitiesConnection=shadow`; off by default. It never changes a response.
 */

import {countLight, type EntityShadowDeps} from "./entityShadow"
import type {FieldShadow} from "./gateway"
import {analyzeLightEntities, noteUnsupported, ORDERED_ROOT, runLight} from "./lightShadow"
import {activateGatewayMetrics, recordGatewayShadow, recordGatewayUnsupported} from "./metrics"

export const ENTITIES_CONNECTION_FIELD = "entitiesConnection"

/** The `entitiesOrderedByProperty` field: a database function answering a plain list, shadowed like `entities`. */
export const ENTITIES_ORDERED_FIELD = ORDERED_ROOT

/** The `entitiesConnection` shadow. Registers its metric series; never throws. */
export const createEntitiesConnectionShadow = (deps: EntityShadowDeps): FieldShadow =>
	createLightListShadow(ENTITIES_CONNECTION_FIELD, {connection: true}, deps)

/**
 * The `entitiesOrderedByProperty` shadow: the returned entities are judged on their cards; the property ordering, the
 * vote and id narrowing, and the page's completeness stay uncompared (lightShadow.ts, readOrderedArgs).
 */
export const createEntitiesOrderedShadow = (deps: EntityShadowDeps): FieldShadow =>
	createLightListShadow(ENTITIES_ORDERED_FIELD, {ordered: true}, deps)

function createLightListShadow(
	field: string,
	mode: {connection?: boolean; ordered?: boolean},
	deps: EntityShadowDeps,
): FieldShadow {
	activateGatewayMetrics(field)
	const label = `gateway ${field} shadow`
	const shadow: FieldShadow = async ({params, document, classification, response}) => {
		try {
			const analysis = analyzeLightEntities(document, classification.operationName, params.variables, {
				...mode,
				entitiesBatch: deps.entitiesBatch === true,
				relationsTypeFilter: deps.relationsTypeFilter === true,
				derivedFields: deps.derivedFields === true,
			})
			if (!analysis.ok) {
				recordGatewayShadow(field, "unsupported_selection")
				recordGatewayUnsupported(field, analysis.reason)
				noteUnsupported(deps, field, analysis.reason, document, classification.operationName, params.variables)
				return
			}
			let body: unknown
			try {
				body = await response.json()
			} catch {
				return
			}
			countLight(deps, field, await runLight(deps, analysis.plan, body, label), {
				operationName: classification.operationName,
			})
		} catch (error) {
			// The response is already gone; count it and move on.
			try {
				recordGatewayShadow(field, "failed")
				deps.log.warn(`${label} errored`, {error: String(error)})
			} catch {}
		}
	}
	// A `metas` cursor sentinel beside `entitiesConnection` is tolerated (counted `metas_cursor`).
	if (mode.connection) shadow.tolerates = ["metas"]
	return shadow
}
