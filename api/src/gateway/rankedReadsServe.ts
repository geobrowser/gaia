import type {FieldNode} from "graphql"
import type {RankedReadsServe, ScoresPorts, TypeCountsPorts} from "../geodb/rankedReadsServe"
import {serveBestOrder, serveEntityColumnsOf, serveTypeCounts} from "../geodb/rankedReadsServe"
import {argsOf, ENTITY_FIELD, fieldsOf, parseOperation, responseKeyOf, Unsupported, uuidArg} from "./entityShadow"
import type {FieldResolver, ResolveResult} from "./gateway"

/**
 * The gateway resolvers for the ranked reads that GEODB_RANKED_READS_SERVE lists (geodb/rankedReadsServe.ts). Each answers one
 * root field, only when its single root field is that field, every argument is one the SQL function takes and every selected field is
 * one the twin can fill exactly, and otherwise declines so PostGraphile answers. A listed read still runs through `serve.run`, so the
 * lag guard, the timeout and the fallback counters apply; a field is served only when it is listed there AND in mode "on" in
 * GRAPHQL_GATEWAY_FIELDS.
 *
 *  - `entitiesInBestOrder(entityIds: [UUID]) { id __typename }`: the page order from the scores geo-db serves.
 *  - `topicFeedTypeCounts(topicIds, typeIds, spaceIds, matchAll, debateTaggedClaims) { typeId entityCount __typename }`.
 *
 *  - `entity(id) { id __typename upvotes downvotes agrees disagrees commenters trendingSource trendingTopicId }` (`entityColumns`): the
 *    count and text columns of one entity, the ones whose Postgres text a count or a label reproduces exactly. The numeric score columns
 *    (`rankingScore`, `qualityScore` ...) are `numeric` in Postgres and an f64 in the twin, so their text can differ in the last digits and
 *    they decline. It shares the `entity` root with the entity-card resolver; `entityColumnsFirst` tries this one and falls through.
 *
 * Reads with no seam here: the `entities_*` columns inside a list (`entities`, `entitiesConnection`, a nested `Entity`), the numeric
 * score columns, and the trending and feed reads, which have no geo-db row source (the twins run over Postgres rows).
 */

export const ENTITIES_IN_BEST_ORDER_FIELD = "entitiesInBestOrder"
export const TOPIC_FEED_TYPE_COUNTS_FIELD = "topicFeedTypeCounts"

/** `entities_in_best_order` takes at most 1000 ids (it raises 22023 above); a longer list is PostGraphile's error to give. */
const MAX_BEST_ORDER_IDS = 1000
const FELL_BACK = Symbol("fell back")

const decline = (reason: string): ResolveResult => ({ok: false, reason})

function rootOf(
	field: string,
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
) {
	const {fragments, variables: vars, roots} = parseOperation(document, operationName, variables)
	const [root] = roots
	if (roots.length !== 1 || !root || root.name.value !== field) throw new Unsupported("not_single_root")
	if (!root.selectionSet) throw new Unsupported("selection")
	return {root, fragments, vars}
}

/** The root's selected fields as response key and name; only the names in `allowed`, plain (no arguments, no sub-selection). */
function plainSelection<N extends string>(
	root: FieldNode,
	fragments: Parameters<typeof fieldsOf>[1],
	allowed: readonly N[],
): {key: string; name: N}[] {
	const out: {key: string; name: N}[] = []
	for (const f of fieldsOf(root.selectionSet as NonNullable<FieldNode["selectionSet"]>, fragments)) {
		const name = f.name.value as N
		if (!allowed.includes(name) || f.arguments?.length || f.selectionSet) throw new Unsupported("selection")
		out.push({key: responseKeyOf(f), name})
	}
	if (out.length === 0) throw new Unsupported("selection")
	return out
}

function uuidList(value: unknown): string[] {
	if (!Array.isArray(value)) throw new Unsupported("args")
	return value.map(uuidArg)
}

function onlyArgs(args: Record<string, unknown>, allowed: readonly string[]) {
	for (const name of Object.keys(args)) if (!allowed.includes(name)) throw new Unsupported("args")
}

// ---- entitiesInBestOrder ------------------------------------------------------------------------------------------------

export type BestOrderPlan = {key: string; ids: string[]; fields: {key: string; name: "id" | "__typename"}[]}

export function planBestOrder(
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
): BestOrderPlan {
	const {root, fragments, vars} = rootOf(ENTITIES_IN_BEST_ORDER_FIELD, document, operationName, variables)
	const args = argsOf(root, vars)
	onlyArgs(args, ["entityIds"])
	if (!("entityIds" in args)) throw new Unsupported("args")
	const ids = uuidList(args.entityIds)
	return {key: responseKeyOf(root), ids, fields: plainSelection(root, fragments, ["id", "__typename"] as const)}
}

/** `entitiesInBestOrder`: PostGraphile's list of `Entity`, `id` as 32 hex, in the order geo-db's scores give. */
export function createBestOrderResolver(serve: RankedReadsServe, ports: ScoresPorts): FieldResolver {
	return async ({params, document}) => {
		let plan: BestOrderPlan
		try {
			plan = planBestOrder(document, params.operationName, params.variables ?? {})
		} catch (error) {
			if (error instanceof Unsupported) return decline(error.reason)
			throw error
		}
		if (plan.ids.length > MAX_BEST_ORDER_IDS) return decline("truncated")
		const order = await serve.run<string[] | typeof FELL_BACK>(
			"bestOrder",
			() => serveBestOrder(ports, plan.ids),
			async () => FELL_BACK,
		)
		if (order === FELL_BACK) return decline("ranked_read_fallback")
		const rows = order.map((id) => {
			const row: Record<string, unknown> = {}
			for (const f of plan.fields) row[f.key] = f.name === "id" ? id : "Entity"
			return row
		})
		return {ok: true, data: {[plan.key]: rows}}
	}
}

// ---- topicFeedTypeCounts ------------------------------------------------------------------------------------------------

type CountsField = "typeId" | "entityCount" | "__typename"
export type TypeCountsServePlan = {
	key: string
	call: {topics: string[]; types: string[]; spaces: string[] | null; matchAll: boolean; debateTaggedClaims: boolean}
	fields: {key: string; name: CountsField}[]
}

export function planTypeCountsServe(
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
): TypeCountsServePlan {
	const {root, fragments, vars} = rootOf(TOPIC_FEED_TYPE_COUNTS_FIELD, document, operationName, variables)
	const args = argsOf(root, vars)
	onlyArgs(args, ["topicIds", "typeIds", "spaceIds", "matchAll", "debateTaggedClaims"])
	if (!("topicIds" in args) || !("typeIds" in args)) throw new Unsupported("args")
	// An omitted argument takes the SQL function's default (spaces NULL, match_all true, debate_tagged_claims false); an explicit null
	// is something else, so only a boolean is read for the flags.
	const flag = (name: "matchAll" | "debateTaggedClaims", fallback: boolean) => {
		if (!(name in args) || args[name] === undefined) return fallback
		if (typeof args[name] !== "boolean") throw new Unsupported("args")
		return args[name] as boolean
	}
	return {
		key: responseKeyOf(root),
		call: {
			topics: uuidList(args.topicIds),
			types: uuidList(args.typeIds),
			spaces: args.spaceIds === undefined || args.spaceIds === null ? null : uuidList(args.spaceIds),
			matchAll: flag("matchAll", true),
			debateTaggedClaims: flag("debateTaggedClaims", false),
		},
		fields: plainSelection(root, fragments, ["typeId", "entityCount", "__typename"] as const),
	}
}

/**
 * `topicFeedTypeCounts`: PostGraphile's list of `TopicFeedTypeCountsRecord`, `typeId` as 32 hex and the bigint `entityCount` as a
 * string, zero counts absent. The SQL's `GROUP BY` fixes no row order, so rows come back by type id; a caller keyed by type is unaffected.
 */
export function createTypeCountsResolver(serve: RankedReadsServe, ports: TypeCountsPorts): FieldResolver {
	return async ({params, document}) => {
		let plan: TypeCountsServePlan
		try {
			plan = planTypeCountsServe(document, params.operationName, params.variables ?? {})
		} catch (error) {
			if (error instanceof Unsupported) return decline(error.reason)
			throw error
		}
		const counts = await serve.run<Map<string, number> | typeof FELL_BACK>(
			"typeCounts",
			() => serveTypeCounts(ports, plan.call),
			async () => FELL_BACK,
		)
		if (counts === FELL_BACK) return decline("ranked_read_fallback")
		const rows = [...counts]
			.map(([type, count]) => [type.replace(/-/g, "").toLowerCase(), count] as const)
			.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
			.map(([type, count]) => {
				const row: Record<string, unknown> = {}
				for (const f of plan.fields)
					row[f.key] =
						f.name === "typeId"
							? type
							: f.name === "entityCount"
								? String(count)
								: "TopicFeedTypeCountsRecord"
				return row
			})
		return {ok: true, data: {[plan.key]: rows}}
	}
}

// ---- entity(id) score columns -------------------------------------------------------------------------------------------

/** GraphQL field -> the SQL function suffix, for the columns served exactly: bigint counts (as strings) and text. */
const COLUMN_OF = {
	upvotes: "upvotes",
	downvotes: "downvotes",
	agrees: "agrees",
	disagrees: "disagrees",
	commenters: "commenters",
	trendingSource: "trending_source",
	trendingTopicId: "trending_topic_id",
} as const
type ColumnField = keyof typeof COLUMN_OF
const COUNT_FIELDS: readonly string[] = ["upvotes", "downvotes", "agrees", "disagrees", "commenters"]
const ENTITY_COLUMN_FIELDS = ["id", "__typename", ...Object.keys(COLUMN_OF)] as readonly (
	| "id"
	| "__typename"
	| ColumnField
)[]

export type EntityColumnsPlan = {
	key: string
	id: string
	fields: {key: string; name: "id" | "__typename" | ColumnField}[]
}

export function planEntityColumns(
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
): EntityColumnsPlan {
	const {root, fragments, vars} = rootOf(ENTITY_FIELD, document, operationName, variables)
	const args = argsOf(root, vars)
	onlyArgs(args, ["id"])
	if (!("id" in args)) throw new Unsupported("args")
	const fields = plainSelection(root, fragments, ENTITY_COLUMN_FIELDS)
	// A selection with no score column is the entity-card resolver's (or PostGraphile's) to answer.
	if (!fields.some((f) => f.name in COLUMN_OF)) throw new Unsupported("selection")
	return {key: responseKeyOf(root), id: uuidArg(args.id), fields}
}

/** `entity(id)` with only the exact score columns: PostGraphile's `Entity` (or null), counts as strings, `trendingTopicId` as 32 hex. */
export function createEntityColumnsResolver(serve: RankedReadsServe, ports: ScoresPorts): FieldResolver {
	return async ({params, document}) => {
		let plan: EntityColumnsPlan
		try {
			plan = planEntityColumns(document, params.operationName, params.variables ?? {})
		} catch (error) {
			if (error instanceof Unsupported) return decline(error.reason)
			throw error
		}
		const columns = await serve.run<Awaited<ReturnType<typeof serveEntityColumnsOf>> | typeof FELL_BACK>(
			"entityColumns",
			() => serveEntityColumnsOf(ports, plan.id),
			async () => FELL_BACK,
		)
		if (columns === FELL_BACK) return decline("ranked_read_fallback")
		if (columns === null) return {ok: true, data: {[plan.key]: null}}
		const row: Record<string, unknown> = {}
		for (const f of plan.fields) {
			if (f.name === "id") row[f.key] = plan.id
			else if (f.name === "__typename") row[f.key] = "Entity"
			else {
				const value = columns[COLUMN_OF[f.name]]
				if (value === null) row[f.key] = null
				else if (COUNT_FIELDS.includes(f.name)) row[f.key] = String(value)
				else if (f.name === "trendingTopicId") row[f.key] = String(value).replace(/-/g, "").toLowerCase()
				else row[f.key] = value
			}
		}
		return {ok: true, data: {[plan.key]: row}}
	}
}

/**
 * Both resolvers want the `entity` root, and the gateway keeps one resolver per root field. This one tries the score-column resolver
 * first (it declines at once for any selection with no score column, without touching geo-db) and then the entity-card resolver.
 */
export function entityColumnsFirst(columns: FieldResolver, entity: FieldResolver | undefined): FieldResolver {
	return async (context) => {
		const out = await columns(context)
		if (out.ok || entity === undefined) return out
		const next = await entity(context)
		return next.ok ? next : out.reason === "selection" ? next : out
	}
}
