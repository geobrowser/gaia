/**
 * The `entitiesConnection` shadow: the connection form of `entities` (`entitiesConnection(first, after, filter,
 * orderBy, spaceIds, typeIds) { nodes { <light entity selection> } totalCount pageInfo }`).
 *
 * PostGraphile answers; for a sampled share of operations geo-db is asked about a few of the returned `nodes`, each
 * judged on its own card exactly as the light `entities` shadow judges them (lightShadow.ts). What the page holds
 * and its order stay uncompared (`filter_completeness`, `order_completeness`), `totalCount` is `total_count`, and
 * `pageInfo` is not read. A page after a cursor (`after`) is never placed in the order. `edges { node { .. } cursor }`
 * is read as the same nodes. An operation that asks only `totalCount` (one root, or several aliased roots) holds no
 * node to judge: with `GEODB_COUNT_ONLY_SHADOW=on` it is lowered to geo-db's entities count form and the `totalCount`s are
 * compared (countOnlyShadow.ts); a shape the form cannot state, or the flag off, is `unsupported_selection{count_only}`.
 *
 * It only runs when GRAPHQL_GATEWAY_FIELDS names `entitiesConnection=shadow`; off by default. It never changes a response.
 */

import {checkCountOnly} from "./countOnlyShadow"
import {countLight, type EntityShadowDeps} from "./entityShadow"
import type {FieldShadow} from "./gateway"
import {
	analyzeCountOnly,
	analyzeLightEntities,
	analyzeLightEntitiesConnection,
	noteUnsupported,
	ORDERED_CONNECTION_ROOT,
	ORDERED_ROOT,
	runLight,
	runLightEntitiesMulti,
} from "./lightShadow"
import {activateGatewayMetrics, recordGatewayShadow, recordGatewayUnsupported} from "./metrics"

export const ENTITIES_CONNECTION_FIELD = "entitiesConnection"

/** The `entitiesOrderedByProperty` field: a database function answering a plain list, shadowed like `entities`. */
export const ENTITIES_ORDERED_FIELD = ORDERED_ROOT

/** The cursor-paginated `entitiesOrderedByPropertyConnection` field: the web's property-ordered feeds and lists. */
export const ENTITIES_ORDERED_CONNECTION_FIELD = ORDERED_CONNECTION_ROOT

/** The `entitiesConnection` shadow. Registers its metric series; never throws. */
export const createEntitiesConnectionShadow = (deps: EntityShadowDeps): FieldShadow =>
	createLightListShadow(ENTITIES_CONNECTION_FIELD, {connection: true}, deps)

/**
 * The `entitiesOrderedByProperty` shadow: the returned entities are judged on their cards; the property ordering, the
 * vote and id narrowing, and the page's completeness stay uncompared (lightShadow.ts, readOrderedArgs).
 */
export const createEntitiesOrderedShadow = (deps: EntityShadowDeps): FieldShadow =>
	createLightListShadow(ENTITIES_ORDERED_FIELD, {ordered: true}, deps)

/**
 * The `entitiesOrderedByPropertyConnection` shadow: `nodes` (or `edges { node }`) are judged on their cards and, for a
 * Space-scoped page, their order is held to geo-db's values, exactly as `entitiesOrderedByProperty`'s list is. `pageInfo`
 * is not read, `totalCount` is `total_count`, and the vote / entity-id narrowing stays `filter_completeness`. A count-only
 * selection is `unsupported_selection{count_only}` (the count-only form states only `entitiesConnection`).
 */
export const createEntitiesOrderedConnectionShadow = (deps: EntityShadowDeps): FieldShadow =>
	createLightListShadow(ENTITIES_ORDERED_CONNECTION_FIELD, {ordered: true, connection: true}, deps)

function createLightListShadow(
	field: string,
	mode: {connection?: boolean; ordered?: boolean},
	deps: EntityShadowDeps,
): FieldShadow {
	activateGatewayMetrics(field)
	const label = `gateway ${field} shadow`
	const isEntitiesConnection = mode.connection === true && !mode.ordered
	const shadow: FieldShadow = async ({params, document, classification, response}) => {
		try {
			const opts = {
				...mode,
				entitiesBatch: deps.entitiesBatch === true,
				relationsTypeFilter: deps.relationsTypeFilter === true,
				derivedFields: deps.derivedFields === true,
			}
			const analysis = isEntitiesConnection
				? analyzeLightEntitiesConnection(document, classification.operationName, params.variables, opts)
				: analyzeLightEntities(document, classification.operationName, params.variables, opts)
			if (!analysis.ok && analysis.reason === "count_only" && isEntitiesConnection && deps.countOnly === true) {
				const counted = analyzeCountOnly(document, classification.operationName, params.variables)
				if (counted.ok) {
					let countBody: unknown
					try {
						countBody = await response.json()
					} catch {
						return
					}
					countLight(deps, field, await checkCountOnly(deps, counted.plan, countBody, label), {
						operationName: classification.operationName,
					})
					return
				}
			}
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
			const outcome =
				"plans" in analysis.plan
					? await runLightEntitiesMulti(deps, analysis.plan, body, label)
					: await runLight(deps, analysis.plan, body, label)
			countLight(deps, field, outcome, {
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
	// Multi-root documents holding `entitiesConnection` (beside `metas` or any other root field) are shadowed independently.
	if (isEntitiesConnection) shadow.tolerates = "*"
	return shadow
}
