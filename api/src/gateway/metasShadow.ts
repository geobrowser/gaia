/**
 * The `metas` shadow: the indexer-cursor root (`metas { nodes { id cursor blockNumber } }`, a page's freshness sentinel).
 *
 * The rows are the pipeline's substreams cursors and block numbers, written to Postgres by two indexers (hermes-pipeline
 * and hermes-ipfs-cache). geo-db's home for them is derived collection 10, `cursors` (plan 10.14 section 1, V1): one row
 * per writer id, group 0, value `<decimal block>:<opaque cursor>`, mirrored flag-off by `geodb-cursor` after each
 * Postgres cursor write (`GEODB_CURSOR_MIRROR`).
 *
 * Without a derived reader (the default) the shadow only counts every sampled operation
 * `unsupported_selection{no_geodb_home}` with its argument shape, so the root is in the coverage numbers. With one it asks
 * the collection for each returned row (up to `MAX_SAMPLED_ROWS`) and compares:
 * - the row is held (else `not_mirrored`: the mirror is off or has not written it yet);
 * - geo-db's block is not behind PostgreSQL's (a lagging or dropped mirror write is a `mismatch`);
 * - at the same block the cursor is equal. At a later block (geo-db was read a moment after PostgreSQL) the cursor is
 *   expected to differ: freshness only, counted uncompared `mirror_newer`.
 * Which indexers the page holds is PostGraphile's (`meta_filter` when an argument narrows it).
 *
 * It only runs when GRAPHQL_GATEWAY_FIELDS names `metas=shadow`; off by default. It never changes a response and logs
 * counts only (no id, cursor or block). Beside `entitiesConnection` the other shadow takes the operation and counts the
 * sentinel as `metas_cursor` (this shadow tolerates no other root).
 */

import type {SelectionSetNode} from "graphql"
import type {DerivedReader} from "../geodb/derived"
import {argsOf, fieldsOf, parseOperation, responseKeyOf, Unsupported} from "./entityShadow"
import type {FieldShadow} from "./gateway"
import {noteUnsupported} from "./lightShadow"
import {
	activateGatewayMetrics,
	recordGatewayShadow,
	recordGatewayUncompared,
	recordGatewayUnsupported,
	type UncomparedReason,
	type UnsupportedReason,
} from "./metrics"

export const METAS_FIELD = "metas"
/** geo-db's registry slot for `cursors` (plan 10.14 V10). */
export const CURSORS_COLLECTION = 10
/** Most returned rows one operation compares. */
export const MAX_SAMPLED_ROWS = 8

type Deps = Parameters<typeof noteUnsupported>[0] & {
	/** Reads derived collection 10. Absent: every operation is counted unsupported (`no_geodb_home`). */
	reader?: DerivedReader
}

export type MetasPlan = {
	/** Response key of the root, and of its `nodes`. */
	key: string
	nodesKey: string
	idKey: string
	cursorKey: string | null
	blockKey: string | null
	uncompared: UncomparedReason[]
}
export type MetasAnalysis = {ok: true; plan: MetasPlan} | {ok: false; reason: UnsupportedReason}

const hex = (text: string) => Buffer.from(text, "utf8").toString("hex")

/** Plans the comparison of a `metas` operation, or says why geo-db cannot be asked. Never throws. */
export function analyzeMetas(
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
): MetasAnalysis {
	try {
		const {fragments, variables: resolved, roots} = parseOperation(document, operationName, variables)
		const root = roots[0]
		if (!root || roots.length !== 1 || root.name.value !== METAS_FIELD)
			return {ok: false, reason: "not_single_root"}
		if (root.directives?.length) return {ok: false, reason: "directives"}
		const uncompared = new Set<UncomparedReason>()
		if (Object.keys(argsOf(root, resolved)).length > 0) uncompared.add("meta_filter")
		const set = root.selectionSet
		if (!set) return {ok: false, reason: "selection"}
		let nodes: SelectionSetNode | null = null
		let nodesKey = ""
		for (const field of fieldsOf(set, fragments)) {
			const name = field.name.value
			if (field.directives?.length) return {ok: false, reason: "directives"}
			if (name === "__typename" || name === "pageInfo") continue
			if (name === "totalCount") uncompared.add("total_count")
			else if (name === "nodes" && field.selectionSet && !field.arguments?.length) {
				nodes = field.selectionSet
				nodesKey = responseKeyOf(field)
			} else return {ok: false, reason: "selection"}
		}
		if (!nodes) return {ok: false, reason: "selection"}
		let idKey: string | null = null
		let cursorKey: string | null = null
		let blockKey: string | null = null
		for (const field of fieldsOf(nodes, fragments)) {
			const name = field.name.value
			if (field.directives?.length) return {ok: false, reason: "directives"}
			if (name === "__typename") continue
			if (field.arguments?.length) return {ok: false, reason: "args"}
			if (name === "id") idKey = responseKeyOf(field)
			else if (name === "cursor") cursorKey = responseKeyOf(field)
			else if (name === "blockNumber") blockKey = responseKeyOf(field)
			else return {ok: false, reason: "selection"}
		}
		if (idKey === null) return {ok: false, reason: "selection"}
		return {
			ok: true,
			plan: {key: responseKeyOf(root), nodesKey, idKey, cursorKey, blockKey, uncompared: [...uncompared]},
		}
	} catch (error) {
		if (error instanceof Unsupported) return {ok: false, reason: error.reason}
		return {ok: false, reason: "selection"}
	}
}

export type MetasOutcome =
	| {result: "match" | "mismatch"; uncompared: UncomparedReason[]; detail: Record<string, unknown>}
	| {result: "unsupported"; reason: UnsupportedReason}
	| {result: "failed"}
	| {result: "skip"}

/** `<decimal block>:<cursor>` as the mirror writes it, or null when the bytes are not that. */
export function parseCursorValue(value: Buffer): {block: bigint; cursor: string} | null {
	const m = /^(\d{1,30}):([\s\S]*)$/.exec(value.toString("utf8"))
	return m ? {block: BigInt(m[1] as string), cursor: m[2] as string} : null
}

const record = (v: unknown): Record<string, unknown> | null =>
	typeof v === "object" && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : null

/** Compares PostGraphile's `body` with the `cursors` collection. Never throws. */
export async function runMetas(reader: DerivedReader, plan: MetasPlan, body: unknown): Promise<MetasOutcome> {
	try {
		const data = record(record(body)?.data)
		const nodes = record(data?.[plan.key])?.[plan.nodesKey]
		if (!Array.isArray(nodes)) return {result: "skip"}
		const rows = nodes
			.map(record)
			.filter((r): r is Record<string, unknown> => r !== null && typeof r[plan.idKey] === "string")
		if (rows.length === 0) return {result: "unsupported", reason: "selection"}
		const uncompared = [...plan.uncompared]
		let mismatches = 0
		let compared = 0
		for (const row of rows.slice(0, MAX_SAMPLED_ROWS)) {
			const held = await reader.get(CURSORS_COLLECTION, hex(row[plan.idKey] as string))
			if (held === null) return {result: "unsupported", reason: "not_mirrored"}
			const mirrored = parseCursorValue(held.value)
			if (mirrored === null) return {result: "unsupported", reason: "bad_layout"}
			const pgBlock =
				plan.blockKey !== null &&
				typeof row[plan.blockKey] === "string" &&
				/^\d{1,30}$/.test(row[plan.blockKey] as string)
					? BigInt(row[plan.blockKey] as string)
					: null
			const pgCursor =
				plan.cursorKey !== null && typeof row[plan.cursorKey] === "string"
					? (row[plan.cursorKey] as string)
					: null
			compared++
			if (pgBlock !== null && mirrored.block < pgBlock) mismatches++
			else if (pgBlock !== null && mirrored.block > pgBlock) uncompared.push("mirror_newer")
			else if (pgCursor !== null && mirrored.cursor !== pgCursor) {
				// Without PostgreSQL's block the cursor cannot be judged: geo-db may simply be later.
				if (pgBlock === null) uncompared.push("mirror_newer")
				else mismatches++
			}
		}
		if (rows.length > MAX_SAMPLED_ROWS) uncompared.push("sampled_rows")
		return {result: mismatches > 0 ? "mismatch" : "match", uncompared, detail: {rows: compared, mismatches}}
	} catch {
		return {result: "failed"}
	}
}

/** The `metas` shadow. Registers its metric series; never throws. */
export function createMetasShadow(deps: Deps): FieldShadow {
	activateGatewayMetrics(METAS_FIELD)
	const unsupported = (
		reason: UnsupportedReason,
		document: Parameters<typeof noteUnsupported>[3],
		operationName: string | null,
		variables: Record<string, unknown>,
	) => {
		recordGatewayShadow(METAS_FIELD, "unsupported_selection")
		recordGatewayUnsupported(METAS_FIELD, reason)
		noteUnsupported(deps, METAS_FIELD, reason, document, operationName, variables)
	}
	return async ({params, document, classification, response}) => {
		try {
			const {reader} = deps
			if (!reader) return unsupported("no_geodb_home", document, classification.operationName, params.variables)
			const analysis = analyzeMetas(document, classification.operationName, params.variables)
			if (!analysis.ok)
				return unsupported(analysis.reason, document, classification.operationName, params.variables)
			let body: unknown
			try {
				body = await response.json()
			} catch {
				return
			}
			const outcome = await runMetas(reader, analysis.plan, body)
			if (outcome.result === "skip") return
			if (outcome.result === "failed") return void recordGatewayShadow(METAS_FIELD, "failed")
			if (outcome.result === "unsupported")
				return unsupported(outcome.reason, document, classification.operationName, params.variables)
			for (const reason of new Set(outcome.uncompared)) recordGatewayUncompared(METAS_FIELD, reason)
			recordGatewayShadow(METAS_FIELD, outcome.result)
			if (outcome.result === "mismatch")
				deps.log.warn("gateway metas shadow mismatch", {
					operationName: classification.operationName,
					...outcome.detail,
				})
		} catch {
			try {
				recordGatewayShadow(METAS_FIELD, "failed")
			} catch {}
		}
	}
}
