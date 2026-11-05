/**
 * The `entity` shadow for an entity's backlinks (`EntityBacklinksPage`):
 *
 *   entity(id: $id) { backlinksList(filter: {spaceId: {is: $spaceId}}) { spaceId fromEntity { id ... } } }
 *
 * PostGraphile answers the relations whose target is the entity; geo-db answers the same question with its card's
 * backlinks (`.args.backlinks`, docs/api/03 §1.1): every statement whose value refers to the entity. The two are
 * compared as sets of (Space, referring entity): each PostGraphile row's referrer must be among geo-db's in that
 * Space, and, when neither side was cut short, each of geo-db's among PostGraphile's.
 *
 * What is not compared: the referrer's own fields (`name`, `spaceIds`, `types`; `nested_entity`), and how many
 * relations a referrer holds to the entity (a set, not a multiset). PostGraphile's list carries no relation type,
 * so a derived relation (one an indexer writes straight into Postgres, which geo-db never holds) cannot be told
 * apart and shows as a referrer only PostGraphile has.
 */
import type {DocumentNode, FieldNode} from "graphql"
import {conformingId, DERIVED_PROPERTIES, type GeoBacklinkRow, MEMBERSHIP_PROPERTIES} from "../geodb/card"
import {GeoDbError, MAX_LINE_SPACES} from "../geodb/client"
import {observeGeoDbRead} from "../geodb/metrics"
import {defaultLimitForDepth} from "../kg/paginationCapPlugin"
import {normalizeUuid} from "../utils/uuid"
import {
	argsOf,
	asRecord,
	type EntityShadowDeps,
	fieldsOf,
	parseOperation,
	responseKeyOf,
	Unsupported,
	uuidArg,
} from "./entityShadow"
import type {LightOutcome} from "./lightShadow"
import type {UncomparedReason, UnsupportedReason} from "./metrics"

/** Most backlinks one geo-db read asks for. More than PostGraphile's page, so its page fits inside. */
export const BACKLINKS_LIMIT = 1024
const SAMPLE = 5

export type BacklinksPlan = {
	entityId: string
	/** The response key of the `entity` root and of its `backlinksList`. */
	entityKey: string
	listKey: string
	/** The filter's Space (compact), or null for every Space. */
	spaceId: string | null
	/** Response keys of each row's `spaceId` (null when not selected) and its referrer's id. */
	spaceKey: string | null
	fromKey: string
	fromIdKey: string
	uncompared: UncomparedReason[]
}

export type BacklinksAnalysis = {ok: true; plan: BacklinksPlan} | {ok: false; reason: UnsupportedReason}

const refuse = (reason: UnsupportedReason): never => {
	throw new Unsupported(reason)
}

const plainKeys = (field: FieldNode) => !field.arguments?.length && !field.directives?.length

/** `{spaceId: {is: <uuid> | null}}`, absent or null: the Space it names, or null for every Space. */
function backlinksSpace(filter: unknown): string | null {
	if (filter === undefined || filter === null) return null
	const record = asRecord(filter)
	if (!record) return refuse("filter")
	const keys = Object.keys(record)
	if (keys.length === 0) return null
	if (keys.length !== 1 || keys[0] !== "spaceId") return refuse("filter")
	const clause = record.spaceId
	if (clause === undefined || clause === null) return null
	const operators = asRecord(clause)
	if (!operators) return refuse("filter")
	const opKeys = Object.keys(operators)
	if (opKeys.length === 0) return null
	if (opKeys.length !== 1 || opKeys[0] !== "is") return refuse("filter")
	const is = operators.is
	return is === undefined || is === null ? null : uuidArg(is)
}

/**
 * Plans an `entity(id) { backlinksList(filter?) { spaceId? fromEntity { id ... } } }` operation, or says why it
 * is not one. Nothing else may sit beside `backlinksList`, and the list takes no paging or ordering arguments.
 */
export function analyzeBacklinks(
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown> | undefined,
): BacklinksAnalysis {
	try {
		const {fragments, variables: resolved, roots} = parseOperation(document, operationName, variables ?? {})
		if (roots.length !== 1 || roots[0]?.name.value !== "entity") return refuse("not_single_root")
		const root = roots[0] as FieldNode
		if (root.directives?.length) return refuse("directives")
		const rootArgs = argsOf(root, resolved)
		if (Object.keys(rootArgs).some((name) => name !== "id")) return refuse("args")
		const entityId = uuidArg(rootArgs.id)
		const inner = fieldsOf(root.selectionSet ?? refuse("selection"), fragments).filter(
			(f) => f.name.value !== "__typename",
		)
		if (inner.length !== 1 || inner[0]?.name.value !== "backlinksList") return refuse("selection")
		const list = inner[0] as FieldNode
		if (list.directives?.length) return refuse("directives")
		const listArgs = argsOf(list, resolved)
		if (Object.keys(listArgs).some((name) => name !== "filter")) return refuse("args")
		const spaceId = backlinksSpace(listArgs.filter)

		let spaceKey: string | null = null
		let from: FieldNode | null = null
		for (const field of fieldsOf(list.selectionSet ?? refuse("selection"), fragments)) {
			const name = field.name.value
			if (name === "__typename") continue
			if (name === "spaceId" && plainKeys(field) && spaceKey === null) spaceKey = responseKeyOf(field)
			else if (name === "fromEntity" && !field.arguments?.length && !field.directives?.length && from === null)
				from = field
			else return refuse("selection")
		}
		if (!from) return refuse("selection")
		const fromFields = fieldsOf(from.selectionSet ?? refuse("selection"), fragments).filter(
			(f) => f.name.value !== "__typename",
		)
		const idField = fromFields.find((f) => f.name.value === "id" && plainKeys(f))
		if (!idField) return refuse("selection")
		const uncompared: UncomparedReason[] = []
		if (fromFields.some((f) => f !== idField)) uncompared.push("nested_entity")
		return {
			ok: true,
			plan: {
				entityId,
				entityKey: responseKeyOf(root),
				listKey: responseKeyOf(list),
				spaceId,
				spaceKey,
				fromKey: responseKeyOf(from),
				fromIdKey: responseKeyOf(idField),
				uncompared,
			},
		}
	} catch (error) {
		if (error instanceof Unsupported) return {ok: false, reason: error.reason}
		throw error
	}
}

/** PostGraphile's backlinks: (gaia Space or null, referrer) pairs, and whether its page was full. */
export type PostgresBacklinks = {
	rows: {spaceId: string | null; from: string}[]
	/** Rows whose referrer PostGraphile could not resolve (a dangling relation). */
	dangling: number
	truncated: boolean
}

/** Reads the backlinks out of PostGraphile's answer; null when the entity is absent or the answer malformed. */
export function extractBacklinks(body: unknown, plan: BacklinksPlan): PostgresBacklinks | null {
	const data = asRecord(asRecord(body)?.data)
	const entity = asRecord(data?.[plan.entityKey])
	if (!entity) return null
	const list = entity[plan.listKey]
	if (!Array.isArray(list)) return null
	const rows: PostgresBacklinks["rows"] = []
	let dangling = 0
	for (const item of list) {
		const row = asRecord(item)
		if (!row) return null
		const from = asRecord(row[plan.fromKey])
		const id = from?.[plan.fromIdKey]
		if (typeof id !== "string") {
			dangling++
			continue
		}
		const space = plan.spaceKey === null ? null : row[plan.spaceKey]
		rows.push({spaceId: typeof space === "string" ? normalizeUuid(space) : null, from: normalizeUuid(id)})
	}
	// The list takes no `first`, so PostGraphile gives it the default page of a root-depth collection.
	return {rows, dangling, truncated: list.length >= defaultLimitForDepth(0)}
}

/** Reads geo-db's backlinks of `entityId` (a gaia id) in `geoSpaceIds`. Throws GeoDbError. */
async function readBacklinks(
	deps: EntityShadowDeps,
	entityId: string,
	geoSpaceIds: string[],
): Promise<GeoBacklinkRow[]> {
	const read = deps.geoDb.readBacklinks?.bind(deps.geoDb)
	if (!read) throw new GeoDbError("client_failed", "the geo-db client cannot read backlinks")
	const started = performance.now()
	try {
		const rows = await read({geoSpaceIds, entityId: conformingId("entity", entityId), limit: BACKLINKS_LIMIT})
		observeGeoDbRead((performance.now() - started) / 1000, "ok")
		return rows
	} catch (error) {
		observeGeoDbRead((performance.now() - started) / 1000, "error")
		throw error
	}
}

const pairKey = (space: string, from: string) => `${space}:${from}`

/**
 * Compares PostGraphile's backlinks with geo-db's for one planned operation. Never throws: a failed read is
 * `failed`, a question geo-db cannot be asked (unloaded Space, no budget, flag off) `unsupported`.
 */
export async function runBacklinks(
	deps: EntityShadowDeps,
	plan: BacklinksPlan,
	body: unknown,
	label: string,
): Promise<LightOutcome> {
	if (!deps.geoDb.readBacklinks) return {result: "unsupported", reason: "selection"}
	const postgres = extractBacklinks(body, plan)
	if (!postgres) return {result: "skip"}

	// The Spaces to read, in geo-db ids, and each PostGraphile row's geo-db Space.
	let geoSpaces: string[]
	let oneSpace: string | null = null
	if (plan.spaceId) {
		const geo = await deps.geoSpaceOf(plan.spaceId)
		if (!geo) return {result: "unsupported", reason: "unmapped_space"}
		geoSpaces = [geo]
		oneSpace = geo
		if (deps.budget && !deps.budget.tryAcquire(1)) return {result: "unsupported", reason: "throttled"}
	} else {
		if (!deps.allGeoSpaces || deps.multiSpace !== true) return {result: "unsupported", reason: "no_space"}
		try {
			geoSpaces = await deps.allGeoSpaces()
		} catch (error) {
			deps.log.warn(`${label} space map unreadable`, {entityId: plan.entityId, message: String(error)})
			return {result: "failed"}
		}
		if (geoSpaces.length === 0) return {result: "unsupported", reason: "no_space"}
		if (geoSpaces.length > MAX_LINE_SPACES && deps.everySpaceChunks !== true)
			return {result: "unsupported", reason: "many_spaces"}
		if (deps.everySpaceBudget && !deps.everySpaceBudget.tryAcquire(Math.ceil(geoSpaces.length / MAX_LINE_SPACES)))
			return {result: "unsupported", reason: "throttled"}
	}

	const postgresPairs = new Set<string>()
	const postgresFrom = new Set<string>()
	for (const row of postgres.rows) {
		const from = conformingId("entity", row.from)
		postgresFrom.add(from)
		if (oneSpace) {
			postgresPairs.add(pairKey(oneSpace, from))
			continue
		}
		if (row.spaceId === null) continue
		const geo = await deps.geoSpaceOf(row.spaceId)
		// A Space geo-db has not loaded cannot hold the row: it is not a question geo-db can answer.
		if (!geo) return {result: "unsupported", reason: "unmapped_space"}
		postgresPairs.add(pairKey(geo, from))
	}
	// Without the rows' Spaces, the sets are compared by referrer alone.
	const bySpace = oneSpace !== null || plan.spaceKey !== null

	let geoRows: GeoBacklinkRow[] = []
	let geoTruncated = false
	try {
		// More Spaces than one line names are read line by line; a backlink lives in one Space, so the union is exact.
		for (let at = 0; at < geoSpaces.length; at += MAX_LINE_SPACES) {
			const line = geoSpaces.slice(at, at + MAX_LINE_SPACES)
			const got = await readBacklinks(deps, plan.entityId, line)
			if (got.length >= BACKLINKS_LIMIT) geoTruncated = true
			// A one-Space line prints bare rows, so a lone last Space names itself.
			const only = line.length === 1 && geoSpaces.length > 1 ? line[0] : undefined
			for (const row of got) geoRows.push(only && row.space === undefined ? {...row, space: only} : row)
		}
	} catch (error) {
		const reason = error instanceof GeoDbError ? error.reason : "error"
		if (reason === "unmapped_space") return {result: "unsupported", reason: "unmapped_space"}
		deps.log.warn(`${label} backlinks read failed`, {
			entityId: plan.entityId,
			spaces: geoSpaces.length,
			reason,
			message: String(error),
		})
		return {result: "failed"}
	}
	const geoPairs = new Set<string>()
	const geoFrom = new Set<string>()
	for (const row of geoRows) {
		// Only geo-db holds these: membership references (the feeder's dual write) and derived statements an edit
		// carried. Neither is a relation PostGraphile lists.
		if (MEMBERSHIP_PROPERTIES.has(row.prop) || DERIVED_PROPERTIES.has(row.prop)) continue
		const space = row.space ?? (geoSpaces.length === 1 ? geoSpaces[0] : undefined)
		if (!space) return {result: "failed"}
		geoPairs.add(pairKey(space, row.from))
		geoFrom.add(row.from)
	}

	const [pgSet, geoSet] = bySpace ? [postgresPairs, geoPairs] : [postgresFrom, geoFrom]
	const uncompared = new Set<UncomparedReason>(plan.uncompared)
	// Every row PostGraphile listed must be geo-db's, unless geo-db's answer was cut short.
	const onlyPostgres = geoTruncated ? [] : [...pgSet].filter((k) => !geoSet.has(k))
	if (geoTruncated) uncompared.add("truncated")
	// And every one geo-db holds PostGraphile's, unless PostGraphile's page was full or held a row whose referrer
	// it could not resolve (which may be the one geo-db names).
	const reverse = !postgres.truncated && !geoTruncated && postgres.dangling === 0
	const onlyGeoDb = reverse ? [...geoSet].filter((k) => !pgSet.has(k)) : []
	if (postgres.truncated) uncompared.add("truncated")
	const result = onlyPostgres.length > 0 || onlyGeoDb.length > 0 ? "mismatch" : "match"
	return {
		result,
		uncompared: [...uncompared],
		detail: {
			entityId: plan.entityId,
			spaceId: plan.spaceId,
			postgres: pgSet.size,
			geoDb: geoSet.size,
			geoRows: geoRows.length,
			dangling: postgres.dangling,
			onlyPostgres: onlyPostgres.slice(0, SAMPLE),
			onlyGeoDb: onlyGeoDb.slice(0, SAMPLE),
		},
	}
}
