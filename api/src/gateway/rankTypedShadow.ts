/**
 * The typed ranking-order shadow (GEODB_RANK_TYPED_SHADOW, off by default).
 *
 * `entitiesConnection(orderBy: [RANKING_SCORE_DESC ..])` orders by a correlated subquery on `entity_ranking_scores` (kg/
 * entityOrderByRankingScorePlugin.ts). Every caller found orders a TYPED set: geogenesis `ClaimRecordClaims`,
 * `ClaimRecordDebates` and the tagged-claims browse all send `typeId`. geo-db holds that typed order in derived collection 0
 * (`entity_type_ranking`, plan 10.14 V10): group the type id, key the type id then the entity id (32 bytes), score
 * `ranking_score`. This asks whether collection 0 could answer those pages, without answering any.
 *
 * GEODB_RANK_TYPED_SHADOW: `off` (default; nothing is counted), `count` (only the operation counter below), `on` (the counter
 * and the shadow). The gateway must be on (`GRAPHQL_GATEWAY=on`); no `GRAPHQL_GATEWAY_FIELDS` entry is needed.
 *
 * Traffic: `gaia_api_ranking_score_orders_total{field,order,typed,narrowed,page}` counts every executed root field
 * (`entitiesConnection` or `entities`, honouring `@skip`/`@include`) whose `orderBy` names a RANKING_SCORE_* or QUALITY_SCORE_* key,
 * unsampled. `typed` is single, multi (several types) or none; `narrowed` says the page carries a filter or another narrowing
 * argument; `page` is first or next (an `after` cursor). Labels are closed sets: no operation name, id or value.
 *
 * Shadow: for a sampled share of those operations (and a token budget) PostgreSQL's answer is read back and compared with
 * collection 0, in one of two modes:
 * - `page`: a single type, `orderBy` led by RANKING_SCORE_DESC, no narrowing, no cursor. The first rows of the type's group are
 *   scanned and compared with the page: the same entities in the same order, where entities with an equal score count as a
 *   set (PostgreSQL breaks them by `updatedAt` and id, which geo-db does not hold; `ties`), and entities after the last scored
 *   one are the unscored tail PostgreSQL sorts last (`unscored_tail`). A page shorter than `first` ends the type, so geo-db
 *   must hold no more scored rows than it.
 * - `rows`: any other single-type page (a filter, a Space, a vote narrowing, a cursor). geo-db cannot say which entities
 *   the filter keeps (`filter`) or where the cursor sits (`cursor`), so each of the page's first rows is read from the collection
 *   by key: it must be held, its score must equal the `rankingScore` the page selected (else `no_score_field`) and the scores
 *   must not rise down the page. That is what a filtered page served from the collection would need to be right.
 *
 * `gaia_api_rank_typed_shadow_total{mode,result}`: match, mismatch, unsupported (see below) or failed (a read failed).
 * Unsupported reasons: type_less (no type: the group is the type), multi_type, order_shape (not led by RANKING_SCORE_DESC),
 * no_ids (the nodes select no id), empty_page, deep_page (past what one scan page holds), not_mirrored (the type's group is
 * empty), row_missing (a row PostgreSQL has is not in the collection: the mirror lags or the type row is stale), bad_layout,
 * ties (a tie group runs past the scanned rows) and throttled. It never changes, delays or fails a response.
 */

import {type DocumentNode, type FieldNode, Kind, type SelectionSetNode, valueFromASTUntyped} from "graphql"
import {GeoDbError} from "../geodb/client"
import type {DerivedScan, DerivedScanReader} from "../geodb/derived"
import {isValidUuid, normalizeUuid} from "../utils/uuid"
import type {ClassifiedRequest} from "./classify"
import {isEmptyFilter} from "./entityShadow"
import type {ResponseObserver, ShadowContext} from "./gateway"
import type {ReadBudget} from "./readBudget"

/** geo-db's registry slot for `entity_type_ranking`. */
export const ENTITY_TYPE_RANKING_COLLECTION = 0

export type RankTypedMode = "off" | "count" | "on"

export type RankTypedConfig = {
	mode: RankTypedMode
	/** Fraction of ranking-ordered operations the shadow compares, 0 to 1. */
	sampleRate: number
	/** Sustained comparisons per second, and the most that may be spent at once. */
	perSecond: number
	burst: number
}

const fraction = (raw: string | undefined, fallback: number) => {
	const n = Number(raw)
	return raw !== undefined && raw.trim() !== "" && Number.isFinite(n) && n >= 0 && n <= 1 ? n : fallback
}
const positive = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isFinite(n) && n > 0 ? Math.min(n, max) : fallback
}

export function readRankTypedConfig(env: Record<string, string | undefined>): RankTypedConfig {
	const flag = env.GEODB_RANK_TYPED_SHADOW?.trim().toLowerCase()
	return {
		mode: flag === "on" || flag === "1" || flag === "true" ? "on" : flag === "count" ? "count" : "off",
		sampleRate: fraction(env.GEODB_RANK_TYPED_SHADOW_SAMPLE, 0.25),
		perSecond: positive(env.GEODB_RANK_TYPED_SHADOW_PER_SECOND, 0.5, 10),
		burst: positive(env.GEODB_RANK_TYPED_SHADOW_BURST, 3, 50),
	}
}

// ---------------------------------------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------------------------------------

export const ORDER_FIELDS = ["entitiesConnection", "entities"] as const
export const SCORE_ORDERS = [
	"RANKING_SCORE_DESC",
	"RANKING_SCORE_ASC",
	"QUALITY_SCORE_DESC",
	"QUALITY_SCORE_ASC",
] as const
export const TYPED = ["single", "multi", "none"] as const
export const MODES = ["page", "rows"] as const
export const RESULTS = ["match", "mismatch", "unsupported", "failed"] as const
export const UNSUPPORTED_REASONS = [
	"type_less",
	"multi_type",
	"order_shape",
	"no_ids",
	"empty_page",
	"deep_page",
	"not_mirrored",
	"row_missing",
	"bad_layout",
	"ties",
	"throttled",
] as const
export const UNCOMPARED_REASONS = [
	"filter",
	"cursor",
	"ties",
	"unscored_tail",
	"no_score_field",
	"sampled_rows",
] as const

export type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]
export type UncomparedReason = (typeof UNCOMPARED_REASONS)[number]
type Result = (typeof RESULTS)[number]
type Mode = (typeof MODES)[number]

let counting = false
let shadowing = false
const orders = new Map<string, number>()
const results = new Map<string, number>()
const unsupported = new Map<string, number>()
const uncompared = new Map<string, number>()
const bump = (map: Map<string, number>, key: string) => map.set(key, (map.get(key) ?? 0) + 1)

/** Empty until the observer is mounted, so a flag-off deploy exports nothing; once mounted every series exists at zero. */
export function renderRankTypedMetrics(): string {
	if (!counting) return ""
	const lines = [
		"# HELP gaia_api_ranking_score_orders_total Executed entitiesConnection / entities root fields ordered by a RANKING_SCORE_* or QUALITY_SCORE_* key, by field, order, whether a single type narrows them (typed), whether a filter or other argument narrows them (narrowed) and whether they follow a cursor (page).",
		"# TYPE gaia_api_ranking_score_orders_total counter",
	]
	for (const field of ORDER_FIELDS)
		for (const order of SCORE_ORDERS)
			for (const typed of TYPED)
				for (const narrowed of ["yes", "no"])
					for (const page of ["first", "next"])
						lines.push(
							`gaia_api_ranking_score_orders_total{field="${field}",order="${order}",typed="${typed}",narrowed="${narrowed}",page="${page}"} ${orders.get([field, order, typed, narrowed, page].join("\t")) ?? 0}`,
						)
	if (!shadowing) return lines.join("\n")
	lines.push(
		"# HELP gaia_api_rank_typed_shadow_total Typed ranking-order pages compared with geo-db's derived collection 0, by mode (page: the page's order; rows: each row's score) and result.",
		"# TYPE gaia_api_rank_typed_shadow_total counter",
	)
	for (const m of MODES)
		for (const r of RESULTS)
			lines.push(`gaia_api_rank_typed_shadow_total{mode="${m}",result="${r}"} ${results.get(`${m}\t${r}`) ?? 0}`)
	lines.push(
		"# HELP gaia_api_rank_typed_shadow_unsupported_total Typed ranking-order pages geo-db's collection 0 cannot answer, by reason.",
		"# TYPE gaia_api_rank_typed_shadow_unsupported_total counter",
	)
	for (const r of UNSUPPORTED_REASONS)
		lines.push(`gaia_api_rank_typed_shadow_unsupported_total{reason="${r}"} ${unsupported.get(r) ?? 0}`)
	lines.push(
		"# HELP gaia_api_rank_typed_shadow_uncompared_total Parts of a compared page that were not checked, by reason; a match covers the checked parts only.",
		"# TYPE gaia_api_rank_typed_shadow_uncompared_total counter",
	)
	for (const r of UNCOMPARED_REASONS)
		lines.push(`gaia_api_rank_typed_shadow_uncompared_total{reason="${r}"} ${uncompared.get(r) ?? 0}`)
	return lines.join("\n")
}

export function __resetRankTypedMetricsForTests(): void {
	counting = false
	shadowing = false
	orders.clear()
	results.clear()
	unsupported.clear()
	uncompared.clear()
}

// ---------------------------------------------------------------------------------------------------------
// Reading the operation
// ---------------------------------------------------------------------------------------------------------

export type RankOp = {
	/** The response key of the root field (its alias, or its name). */
	key: string
	field: (typeof ORDER_FIELDS)[number]
	/** The `orderBy` names as sent. */
	orderBy: string[]
	/** The first score-order name in `orderBy`, the counter's label. */
	order: (typeof SCORE_ORDERS)[number]
	typed: (typeof TYPED)[number]
	/** The single type's id, 32 hex digits, when `typed` is single. */
	typeHex: string | null
	cursor: boolean
	/** An argument beyond the type, order, `first` and `after` narrows the set. */
	narrowed: boolean
	first: number | null
}

const SCORE_ORDER = new Set<string>(SCORE_ORDERS)
const PLAIN_ARGS = new Set(["typeId", "typeIds", "orderBy", "first", "after"])
const MAX_DEPTH = 8

/** Whether `@skip`/`@include` leave the selection out. A directive that cannot be read leaves it in. */
function skipped(
	selection: {directives?: readonly {name: {value: string}; arguments?: readonly unknown[]}[]},
	vars: Record<string, unknown>,
) {
	for (const directive of selection.directives ?? []) {
		const name = directive.name.value
		if (name !== "skip" && name !== "include") continue
		const arg = (
			directive.arguments as
				| {name: {value: string}; value: Parameters<typeof valueFromASTUntyped>[0]}[]
				| undefined
		)?.find((a) => a.name.value === "if")
		if (!arg) continue
		const value = valueFromASTUntyped(arg.value, vars)
		if (typeof value !== "boolean") continue
		if (name === "skip" ? value : !value) return true
	}
	return false
}

function rootFields(
	set: SelectionSetNode,
	fragments: Map<string, SelectionSetNode>,
	vars: Record<string, unknown>,
	out: FieldNode[],
	depth = 0,
): void {
	if (depth > MAX_DEPTH) return
	for (const selection of set.selections) {
		if (skipped(selection, vars)) continue
		if (selection.kind === Kind.FIELD) out.push(selection)
		else if (selection.kind === Kind.INLINE_FRAGMENT)
			rootFields(selection.selectionSet, fragments, vars, out, depth + 1)
		else {
			const fragment = fragments.get(selection.name.value)
			if (fragment) rootFields(fragment, fragments, vars, out, depth + 1)
		}
	}
}

const asRecord = (v: unknown): Record<string, unknown> | null =>
	v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null

/** The type ids a `typeIds: {in: [..]}` or `{is: ..}` argument names, or null when it is not that shape. */
function typeIdsOf(value: unknown): string[] | null {
	const clause = asRecord(value)
	if (!clause) return null
	const keys = Object.keys(clause)
	if (keys.length !== 1) return null
	const listed = keys[0] === "in" ? clause.in : keys[0] === "is" ? [clause.is] : null
	if (!Array.isArray(listed) || listed.length === 0) return null
	if (!listed.every((id) => typeof id === "string" && isValidUuid(id))) return null
	return [...new Set(listed.map((id) => normalizeUuid(id as string) as string))]
}

/**
 * The executed root fields of a query that order by a score key. Never throws; an operation that is not a query, or does not
 * parse as one, has none.
 */
export function rankOps(
	document: DocumentNode,
	operationName: string | null,
	variables: Record<string, unknown>,
): RankOp[] {
	try {
		const fragments = new Map<string, SelectionSetNode>()
		const operations = []
		for (const definition of document.definitions) {
			if (definition.kind === Kind.FRAGMENT_DEFINITION)
				fragments.set(definition.name.value, definition.selectionSet)
			else if (definition.kind === Kind.OPERATION_DEFINITION) operations.push(definition)
		}
		const operation = operationName
			? operations.find((op) => op.name?.value === operationName)
			: operations.length === 1
				? operations[0]
				: undefined
		if (!operation || operation.operation !== "query") return []
		const vars = {...variables}
		for (const definition of operation.variableDefinitions ?? []) {
			const name = definition.variable.name.value
			if (vars[name] === undefined && definition.defaultValue)
				vars[name] = valueFromASTUntyped(definition.defaultValue)
		}
		const fields: FieldNode[] = []
		rootFields(operation.selectionSet, fragments, vars, fields)
		const found: RankOp[] = []
		for (const field of fields) {
			const name = field.name.value
			if (name !== "entitiesConnection" && name !== "entities") continue
			const args: Record<string, unknown> = {}
			for (const arg of field.arguments ?? []) args[arg.name.value] = valueFromASTUntyped(arg.value, vars)
			const raw = args.orderBy
			const orderBy = (Array.isArray(raw) ? raw : raw === undefined || raw === null ? [] : [raw]).filter(
				(n): n is string => typeof n === "string",
			)
			const order = orderBy.find((n) => SCORE_ORDER.has(n)) as RankOp["order"] | undefined
			if (!order) continue
			const given = (arg: string) => args[arg] !== undefined && args[arg] !== null
			let types: string[] | null = null
			let typeless = true
			if (given("typeId")) {
				typeless = false
				types = typeof args.typeId === "string" && isValidUuid(args.typeId) ? [normalizeUuid(args.typeId)] : []
			}
			if (given("typeIds")) {
				typeless = false
				const more = typeIdsOf(args.typeIds)
				types = types === null ? (more ?? []) : [...new Set([...types, ...(more ?? [])])]
			}
			const narrowed = Object.keys(args).some(
				(arg) => !PLAIN_ARGS.has(arg) && given(arg) && (arg !== "filter" || !isEmptyFilter(args.filter)),
			)
			const single = !typeless && types?.length === 1
			found.push({
				key: field.alias?.value ?? name,
				field: name,
				orderBy,
				order,
				typed: typeless ? "none" : single ? "single" : "multi",
				typeHex: single ? (types?.[0] ?? null) : null,
				cursor: given("after"),
				narrowed,
				first: typeof args.first === "number" && Number.isInteger(args.first) ? args.first : null,
			})
		}
		return found
	} catch {
		return []
	}
}

/** Counts the operation's ranking-ordered root fields. */
export function countRankOps(ops: readonly RankOp[]): void {
	for (const op of ops)
		bump(
			orders,
			[op.field, op.order, op.typed, op.narrowed ? "yes" : "no", op.cursor ? "next" : "first"].join("\t"),
		)
}

// ---------------------------------------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------------------------------------

/** Scores are f64 on both sides; PostgreSQL's numeric to JSON round trip may differ in the last digits. */
const TOLERANCE = 1e-9
const sameScore = (a: number, b: number) => Math.abs(a - b) <= TOLERANCE * Math.max(1, Math.abs(a), Math.abs(b))

/** The most nodes a compared page may hold; a bigger one is past what one scan page is asked for. */
export const MAX_PAGE = 100
/** Rows scanned past the page, so a small tie group at the cut is seen whole. */
const SCAN_SLACK = 32
/** The rows of a `rows` page read by key. */
export const MAX_ROWS = 10

/** One node of PostgreSQL's page: the entity (32 hex) and the `rankingScore` it selected (null: unscored; undefined: not selected). */
export type PageNode = {id: string; score: number | null | undefined}

export type Outcome =
	| {result: "match" | "mismatch"; uncompared: UncomparedReason[]; detail: Record<string, unknown>}
	| {result: "unsupported"; reason: UnsupportedReason}

const keyOf = (typeHex: string, entityHex: string) => typeHex + entityHex

/**
 * The page compared with the first rows of the type's group, `requested` rows asked for. Pure. See the file header for the
 * rules; `first` is the page size PostgreSQL was asked for (null: unknown).
 */
export function comparePage(
	typeHex: string,
	page: readonly PageNode[],
	first: number | null,
	scan: DerivedScan,
): Outcome {
	const rows: {id: string; score: number}[] = []
	for (const row of scan.rows) {
		if (row.keyHex.length !== 64 || !row.keyHex.startsWith(typeHex) || row.valueHex !== "")
			return {result: "unsupported", reason: "bad_layout"}
		rows.push({id: row.keyHex.slice(32), score: row.score})
	}
	if (rows.length === 0) return {result: "unsupported", reason: "not_mirrored"}
	const extra = new Set<UncomparedReason>()
	const held = new Map(rows.map((r) => [r.id, r.score]))
	if (new Set(page.map((n) => n.id)).size !== page.length) return mismatch("repeated entity")
	let at = 0
	let from = 0
	while (from < rows.length && at < page.length) {
		let to = from + 1
		while (to < rows.length && rows[to]?.score === rows[from]?.score) to++
		const members = new Set(rows.slice(from, to).map((r) => r.id))
		const take = Math.min(to - from, page.length - at)
		if (to - from > 1) extra.add("ties")
		for (let i = at; i < at + take; i++) {
			if (!members.has((page[i] as PageNode).id)) {
				// A tie group cut by the end of the scan may continue in rows not read.
				if (to === rows.length && scan.more) return {result: "unsupported", reason: "ties"}
				return mismatch(`position ${i}: not the geo-db row`)
			}
		}
		at += take
		from = to
	}
	if (at < page.length) {
		// The scored rows ran out inside the page: the rest is the unscored tail PostgreSQL sorts last.
		if (scan.more) return {result: "unsupported", reason: "deep_page"}
		extra.add("unscored_tail")
		for (let i = at; i < page.length; i++) {
			const node = page[i] as PageNode
			if (held.has(node.id)) return mismatch(`position ${i}: scored in geo-db, after the scored rows`)
			if (typeof node.score === "number") return {result: "unsupported", reason: "row_missing"}
		}
	} else if (!scan.more && first !== null && page.length < first && rows.length > page.length) {
		// A page short of `first` is the end of the type, so geo-db holds no scored row PostgreSQL's page lacks.
		return mismatch(`short page of ${page.length}, geo-db holds ${rows.length}`)
	}
	let selected = 0
	for (const node of page) {
		const score = held.get(node.id)
		if (node.score === undefined || score === undefined) continue
		selected++
		if (node.score === null || !sameScore(node.score, score)) return mismatch("score differs")
	}
	if (selected === 0) extra.add("no_score_field")
	return {result: "match", uncompared: [...extra], detail: {postgres: page.length, geoDb: rows.length}}
}

function mismatch(why: string): Outcome {
	return {result: "mismatch", uncompared: [], detail: {why}}
}

/**
 * A narrowed or cursor page's rows, each read by key: `rows[i]` is the collection's row for `page[i]` (null: not held). The
 * order and the scores must agree with the collection. Pure.
 */
export function compareRows(
	page: readonly PageNode[],
	rows: readonly ({score: number} | null)[],
	extra: readonly UncomparedReason[],
): Outcome {
	const uncomparedNow = new Set<UncomparedReason>(extra)
	let last = Number.POSITIVE_INFINITY
	let checked = 0
	for (let i = 0; i < page.length; i++) {
		const node = page[i] as PageNode
		const row = rows[i] ?? null
		if (row === null) {
			// PostgreSQL sorts an unscored entity last; it has no row because it has no score.
			if (node.score === null) continue
			return {result: "unsupported", reason: "row_missing"}
		}
		if (node.score === null) return {result: "mismatch", uncompared: [...uncomparedNow], detail: {unscored: "held"}}
		if (node.score !== undefined && !sameScore(node.score, row.score))
			return {
				result: "mismatch",
				uncompared: [...uncomparedNow],
				detail: {index: i, score: "differs", postgres: node.score, geoDb: row.score},
			}
		if (row.score > last && !sameScore(row.score, last))
			return {result: "mismatch", uncompared: [...uncomparedNow], detail: {index: i, order: "rises"}}
		last = row.score
		checked++
	}
	if (page.some((n) => n.score === undefined)) uncomparedNow.add("no_score_field")
	uncomparedNow.add("sampled_rows")
	return {result: "match", uncompared: [...uncomparedNow], detail: {rows: checked}}
}

// ---------------------------------------------------------------------------------------------------------
// The observer
// ---------------------------------------------------------------------------------------------------------

const hex32 = (id: unknown): string | null =>
	typeof id === "string" && isValidUuid(id) ? (normalizeUuid(id) as string) : null

/** PostgreSQL's answer for one root field, as the nodes the compare reads, or null when the answer has none to read. */
export function nodesOf(op: RankOp, data: unknown): {nodes: unknown[]} | null {
	const root = asRecord(data)?.[op.key]
	const list = op.field === "entities" ? root : asRecord(root)?.nodes
	return Array.isArray(list) ? {nodes: list} : null
}

function pageOf(nodes: unknown[]): PageNode[] | null {
	const page: PageNode[] = []
	for (const node of nodes) {
		const record = asRecord(node)
		const id = hex32(record?.id)
		if (!record || id === null) return null
		const raw = record.rankingScore
		const score = raw === undefined ? undefined : raw === null ? null : Number(raw)
		if (typeof score === "number" && !Number.isFinite(score)) return null
		page.push({id, score})
	}
	return page
}

export type RankTypedDeps = {
	config: RankTypedConfig
	reader: DerivedScanReader
	budget: ReadBudget
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	random?: () => number
}

/** Settles one operation's root field. Never throws. */
export async function checkOp(deps: RankTypedDeps, op: RankOp, data: unknown): Promise<void> {
	const refuse = (reason: UnsupportedReason, mode: Mode = "page") => {
		bump(results, `${mode}\tunsupported`)
		bump(unsupported, reason)
	}
	if (op.typed === "none") return refuse("type_less")
	if (op.typed === "multi" || op.typeHex === null) return refuse("multi_type")
	if (op.orderBy[0] !== "RANKING_SCORE_DESC") return refuse("order_shape")
	const read = nodesOf(op, data)
	if (!read) return
	if (read.nodes.length === 0) return refuse("empty_page")
	if (read.nodes.length > MAX_PAGE || (op.first !== null && op.first > MAX_PAGE)) return refuse("deep_page")
	const page = pageOf(read.nodes)
	if (!page) return refuse("no_ids")
	const exact = !op.narrowed && !op.cursor
	const mode: Mode = exact ? "page" : "rows"
	if (!deps.budget.tryAcquire(1)) return refuse("throttled", mode)
	const typeHex = op.typeHex
	let outcome: Outcome
	try {
		if (exact) {
			const asked = Math.min(512, page.length + SCAN_SLACK)
			outcome = comparePage(
				typeHex,
				page,
				op.first,
				await deps.reader.scan(ENTITY_TYPE_RANKING_COLLECTION, typeHex, asked),
			)
		} else {
			const sample = page.slice(0, MAX_ROWS)
			const rows = []
			for (const node of sample) {
				const row = await deps.reader.get(ENTITY_TYPE_RANKING_COLLECTION, keyOf(typeHex, node.id))
				rows.push(row && row.group === typeHex && row.value.length === 0 ? {score: row.score} : null)
			}
			outcome = compareRows(sample, rows, [
				...(op.narrowed ? (["filter"] as const) : []),
				...(op.cursor ? (["cursor"] as const) : []),
			])
		}
	} catch (error) {
		bump(results, `${mode}\tfailed`)
		if (!(error instanceof GeoDbError)) deps.log.warn("rank typed shadow failed", {error: String(error)})
		return
	}
	if (outcome.result === "unsupported") return refuse(outcome.reason, mode)
	bump(results, `${mode}\t${outcome.result satisfies Result}`)
	for (const reason of outcome.uncompared) bump(uncompared, reason)
	// Counts only: never which entity.
	if (outcome.result === "mismatch") deps.log.warn("rank typed shadow mismatch", {mode, ...outcome.detail})
}

/**
 * The observer the gateway runs on every operation (`GatewayDeps.observers`): counts ranking-ordered root fields, and for a
 * sampled share asks for PostgreSQL's answer to compare. Counts nothing and reads nothing when the mode is `off`.
 */
export function createRankTypedObserver(deps: RankTypedDeps): ResponseObserver {
	const {config} = deps
	const draw = deps.random ?? Math.random
	counting = config.mode !== "off"
	shadowing = config.mode === "on"
	return {
		inspect(classified: ClassifiedRequest) {
			if (config.mode === "off" || !classified.params || !classified.document) return false
			const ops = rankOps(classified.document, classified.params.operationName, classified.params.variables)
			if (ops.length === 0) return false
			countRankOps(ops)
			return config.mode === "on" && draw() < config.sampleRate
		},
		async observe({document, params, response}: ShadowContext) {
			const ops = rankOps(document, params.operationName, params.variables)
			if (ops.length === 0) return
			let body: unknown
			try {
				body = await response.json()
			} catch {
				return
			}
			const data = asRecord(body)?.data
			if (!data) return
			for (const op of ops) {
				try {
					await checkOp(deps, op, data)
				} catch (error) {
					deps.log.warn("rank typed shadow errored", {error: String(error)})
				}
			}
		},
	}
}
