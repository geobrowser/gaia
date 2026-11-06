/**
 * The `spaces` shadow: the list root `spaces(filter, first, offset) { id type address topicId page { id name }
 * topic { id name } members editors .. }` (the Space page, the membership dialogs, the personal-space lookup).
 *
 * PostGraphile answers; for a sampled share of operations geo-db is asked about a few of the returned Spaces:
 * - the Space is held (`geoSpaceOf` maps its id). A Space geo-db has not loaded is not a difference, it is a gap the
 *   feeder knows about, so it is counted `space_not_held` and not compared. If every sampled Space is such a gap the
 *   operation is `unsupported_selection{unmapped_space}`.
 * - the name of its `page` / `topic` entity, when selected: geo-db's card of that entity in that Space holds it.
 *
 * Counted uncompared, by reason: which Spaces the page holds and in what order (`filter_completeness`,
 * `order_completeness`), `members` / `editors` (`space_members`), the Space's own settings and any other nested part of
 * `page` / `topic` (`space_settings`, `nested_entity`).
 *
 * It only runs when GRAPHQL_GATEWAY_FIELDS names `spaces=shadow`; off by default. It never changes a response and
 * logs counts only, never an id or a name.
 */

import {SystemIds} from "@geoprotocol/geo-sdk"
import type {SelectionSetNode} from "graphql"
import {conformingId, type GeoCardRow} from "../geodb/card"
import {GeoDbError} from "../geodb/client"
import {askGeoDb, type CardDeps, type CardLogger} from "../geodb/service"
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

export const SPACES_FIELD = "spaces"

/** Most Spaces of one answer compared. */
export const MAX_SAMPLED_SPACES = 3

const NAME = conformingId("entity", SystemIds.NAME_PROPERTY)

/** Fields of a Space that no entity card holds: counted `space_settings`. */
const SETTINGS = new Set(["type", "address", "topicId", "spaceVotingSetting"])
const MEMBERS = new Set(["members", "editors", "membersList", "editorsList"])

type EntityPart = {key: string; idKey: string}
export type SpacesPlan = {
	/** Response key of the root (an alias, else `spaces`). */
	key: string
	/** Response key of `id`; a selection without it is unsupported. */
	idKey: string
	parts: {page?: EntityPart; topic?: EntityPart}
	uncompared: UncomparedReason[]
}

export type SpacesAnalysis = {ok: true; plan: SpacesPlan} | {ok: false; reason: UnsupportedReason}

const ROOT_ARGS = new Set(["filter", "first", "offset", "orderBy"])

/** Plans the comparison of a `spaces` operation, or says why geo-db cannot be asked. Never throws. */
export function analyzeSpaces(
	document: Parameters<typeof parseOperation>[0],
	operationName: string | null,
	variables: Record<string, unknown>,
): SpacesAnalysis {
	try {
		const {fragments, variables: resolved, roots} = parseOperation(document, operationName, variables)
		const root = roots[0]
		if (!root || roots.length !== 1 || root.name.value !== SPACES_FIELD)
			return {ok: false, reason: "not_single_root"}
		const args = argsOf(root, resolved)
		for (const name of Object.keys(args)) if (!ROOT_ARGS.has(name)) return {ok: false, reason: "args"}
		if (root.directives?.length) return {ok: false, reason: "directives"}
		const set = root.selectionSet
		if (!set) return {ok: false, reason: "selection"}

		const uncompared = new Set<UncomparedReason>()
		// An id filter (`id: {in: [...]}`) is the whole membership of the page only if nothing else filters; any
		// filter or paging leaves "which Spaces" to PostGraphile.
		if (args.filter !== undefined && args.filter !== null) uncompared.add("filter_completeness")
		if (args.first !== undefined || args.offset !== undefined) uncompared.add("filter_completeness")
		if (args.orderBy !== undefined) uncompared.add("order_completeness")

		let idKey: string | null = null
		const parts: SpacesPlan["parts"] = {}
		for (const field of fieldsOf(set, fragments)) {
			const name = field.name.value
			if (field.directives?.length) return {ok: false, reason: "directives"}
			if (name === "__typename") continue
			if (name === "id") {
				if (field.arguments?.length) return {ok: false, reason: "args"}
				idKey = responseKeyOf(field)
			} else if (SETTINGS.has(name)) uncompared.add("space_settings")
			else if (MEMBERS.has(name)) uncompared.add("space_members")
			else if (name === "page" || name === "topic") {
				if (field.arguments?.length || !field.selectionSet) return {ok: false, reason: "args"}
				let nameSelected = false
				let idSelected = false
				for (const inner of fieldsOf(field.selectionSet, fragments)) {
					if (inner.directives?.length) return {ok: false, reason: "directives"}
					const innerName = inner.name.value
					if (innerName === "__typename") continue
					if (innerName === "name" && !inner.arguments?.length) nameSelected = true
					else if (innerName === "id" && !inner.arguments?.length) idSelected = true
					else uncompared.add("nested_entity")
				}
				// The comparison reads the entity's card, so the entity's id is needed from the answer.
				if (nameSelected && !idSelected) return {ok: false, reason: "selection"}
				if (nameSelected)
					parts[name] = {key: responseKeyOf(field), idKey: innerKey(field.selectionSet, fragments, "id")}
				else uncompared.add("nested_entity")
			} else return {ok: false, reason: "selection"}
		}
		if (idKey === null) return {ok: false, reason: "selection"}
		return {ok: true, plan: {key: responseKeyOf(root), idKey, parts, uncompared: [...uncompared]}}
	} catch (error) {
		if (error instanceof Unsupported) return {ok: false, reason: error.reason}
		return {ok: false, reason: "selection"}
	}
}

function innerKey(set: SelectionSetNode, fragments: Map<string, SelectionSetNode>, name: string): string {
	for (const f of fieldsOf(set, fragments)) if (f.name.value === name) return responseKeyOf(f)
	return name
}

const compact = (value: unknown): string | null =>
	typeof value === "string" ? value.replaceAll("-", "").toLowerCase() : null

/** Postgres serves an empty text as a name; geo-db holds no empty text, so both mean none. */
const namesIn = (rows: GeoCardRow[]) => rows.filter((r) => r.prop === NAME && r.raw !== "").map((r) => r.raw)

export type SpacesShadowDeps = Pick<CardDeps, "geoDb" | "geoSpaceOf"> & {log: CardLogger}

type Outcome =
	| {result: "skip"}
	| {result: "failed"}
	| {result: "unsupported"; reason: UnsupportedReason}
	| {result: "match" | "mismatch"; uncompared: UncomparedReason[]; detail: Record<string, number>}

/** Compares up to MAX_SAMPLED_SPACES Spaces of PostGraphile's answer with geo-db. Never throws. */
export async function runSpaces(deps: SpacesShadowDeps, plan: SpacesPlan, body: unknown): Promise<Outcome> {
	const data = (body as {data?: Record<string, unknown>} | null)?.data
	const list = data?.[plan.key]
	if (!Array.isArray(list) || list.length === 0) return {result: "skip"}
	const uncompared = new Set<UncomparedReason>(plan.uncompared)
	const spaces = list.slice(0, MAX_SAMPLED_SPACES)
	let held = 0
	let notHeld = 0
	let nameMismatch = 0
	let namesCompared = 0
	try {
		for (const space of spaces) {
			const record = space as Record<string, unknown> | null
			const spaceId = compact(record?.[plan.idKey])
			if (!spaceId) return {result: "failed"}
			if (!(await deps.geoSpaceOf(spaceId))) {
				notHeld++
				continue
			}
			held++
			for (const kind of ["page", "topic"] as const) {
				const part = plan.parts[kind]
				if (!part) continue
				const entity = record?.[part.key] as Record<string, unknown> | null | undefined
				const entityId = compact(entity?.[part.idKey])
				if (!entity || !entityId) {
					uncompared.add("nested_entity")
					continue
				}
				let rows: GeoCardRow[]
				try {
					rows = await askGeoDb(deps, conformingId("entity", entityId), spaceId)
				} catch (error) {
					if (error instanceof GeoDbError && error.reason === "unmapped_space") {
						notHeld++
						continue
					}
					throw error
				}
				const served = entity.name
				const names = namesIn(rows)
				const ok = typeof served === "string" && served !== "" ? names.includes(served) : names.length === 0
				namesCompared++
				if (!ok) nameMismatch++
			}
		}
	} catch (error) {
		if (!(error instanceof GeoDbError)) deps.log.warn("gateway spaces shadow errored", {error: String(error)})
		return {result: "failed"}
	}
	if (notHeld > 0) uncompared.add("space_not_held")
	if (held === 0) return {result: "unsupported", reason: "unmapped_space"}
	if (list.length > spaces.length) uncompared.add("sampled_rows")
	return {
		result: nameMismatch === 0 ? "match" : "mismatch",
		uncompared: [...uncompared],
		detail: {sampled: spaces.length, held, notHeld, namesCompared, nameMismatch},
	}
}

/** The `spaces` shadow. Registers its metric series; never throws. */
export function createSpacesShadow(deps: SpacesShadowDeps): FieldShadow {
	activateGatewayMetrics(SPACES_FIELD)
	return async ({params, document, classification, response}) => {
		try {
			const analysis = analyzeSpaces(document, classification.operationName, params.variables)
			if (!analysis.ok) {
				recordGatewayShadow(SPACES_FIELD, "unsupported_selection")
				recordGatewayUnsupported(SPACES_FIELD, analysis.reason)
				noteUnsupported(
					deps,
					SPACES_FIELD,
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
			const outcome = await runSpaces(deps, analysis.plan, body)
			if (outcome.result === "skip") return
			if (outcome.result === "failed") return void recordGatewayShadow(SPACES_FIELD, "failed")
			if (outcome.result === "unsupported") {
				recordGatewayShadow(SPACES_FIELD, "unsupported_selection")
				recordGatewayUnsupported(SPACES_FIELD, outcome.reason)
				return
			}
			for (const reason of new Set(outcome.uncompared)) recordGatewayUncompared(SPACES_FIELD, reason)
			recordGatewayShadow(SPACES_FIELD, outcome.result)
			if (outcome.result === "mismatch")
				deps.log.warn("gateway spaces shadow mismatch", {
					operationName: classification.operationName,
					...outcome.detail,
				})
		} catch (error) {
			try {
				recordGatewayShadow(SPACES_FIELD, "failed")
				deps.log.warn("gateway spaces shadow errored", {error: String(error)})
			} catch {}
		}
	}
}
