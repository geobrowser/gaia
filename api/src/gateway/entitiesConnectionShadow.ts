/**
 * The `entitiesConnection` shadow: the connection form of `entities` (`entitiesConnection(first, after, filter,
 * orderBy, spaceIds, typeIds) { nodes { <light entity selection> } totalCount pageInfo }`).
 *
 * PostGraphile answers; for a sampled share of operations geo-db is asked about a few of the returned `nodes`, each
 * judged on its own card exactly as the light `entities` shadow judges them (lightShadow.ts). What the page holds
 * and its order stay uncompared (`filter_completeness`, `order_completeness`), `totalCount` is `total_count`, and
 * `pageInfo` is not read. A page after a cursor (`after`) is never placed in the order.
 *
 * It only runs when GRAPHQL_GATEWAY_FIELDS names `entitiesConnection=shadow`; off by default. It never changes a response.
 */

import {countLight, type EntityShadowDeps} from "./entityShadow"
import type {FieldShadow} from "./gateway"
import {analyzeLightEntities, noteUnsupported, runLight} from "./lightShadow"
import {activateGatewayMetrics, recordGatewayShadow, recordGatewayUnsupported} from "./metrics"

export const ENTITIES_CONNECTION_FIELD = "entitiesConnection"

/** The `entitiesConnection` shadow. Registers its metric series; never throws. */
export function createEntitiesConnectionShadow(deps: EntityShadowDeps): FieldShadow {
	activateGatewayMetrics(ENTITIES_CONNECTION_FIELD)
	const label = "gateway entitiesConnection shadow"
	return async ({params, document, classification, response}) => {
		try {
			const analysis = analyzeLightEntities(document, classification.operationName, params.variables, {
				connection: true,
				entitiesBatch: deps.entitiesBatch === true,
				relationsTypeFilter: deps.relationsTypeFilter === true,
				derivedFields: deps.derivedFields === true,
			})
			if (!analysis.ok) {
				recordGatewayShadow(ENTITIES_CONNECTION_FIELD, "unsupported_selection")
				recordGatewayUnsupported(ENTITIES_CONNECTION_FIELD, analysis.reason)
				noteUnsupported(
					deps,
					ENTITIES_CONNECTION_FIELD,
					analysis.reason,
					document,
					classification.operationName,
					params.variables,
				)
				return
			}
			let body: unknown
			try {
				body = await response.json()
			} catch {
				return
			}
			countLight(deps, ENTITIES_CONNECTION_FIELD, await runLight(deps, analysis.plan, body, label), {
				operationName: classification.operationName,
			})
		} catch (error) {
			// The response is already gone; count it and move on.
			try {
				recordGatewayShadow(ENTITIES_CONNECTION_FIELD, "failed")
				deps.log.warn(`${label} errored`, {error: String(error)})
			} catch {}
		}
	}
}
