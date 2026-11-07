/**
 * The governance queries shadow (GEODB_GOVERNANCE_QUERIES_SHADOW, off by default): the proposal reads the status shadow
 * (shadow.ts) does not cover, twinned from the keyed governance records in geo-db. Same discipline: it runs after the
 * response is built, in the background, bounded by `maxInFlight`, swallows every error, and logs counts only.
 *
 *  - detail: the actions of the current version (type, target, content uri and id, in order) and the vote count of the
 *    current version, from the action records and the vote records of the proposal's backlinks.
 *  - list: the same actions per listed proposal, plus whether geo-db's action types satisfy the actionTypes /
 *    excludeActionTypes filter the request carried.
 *  - active_member / active_editor: whether any not-executed proposal has an AddMember / AddEditor action at its
 *    latest version aimed at the target (backlinks of the target). An upper bound: see `activeFromRecords`.
 *
 * What stays with Postgres is counted in `gaia_api_governance_queries_shadow_uncompared_total`: which proposals a space
 * has (no space index in the records), order and cursor, vote counts on a list, the active checks' space scope and
 * vote tallies, and the UpdateVotingSettings fields of an action.
 */
import {type GeoDbCardClient, GeoDbError} from "../../geodb/client"
import type {ProposalAction, ProposalListItem, ProposalWithVotes} from "../types"
import {
	activateQueriesMetrics,
	type Query,
	type QueryUnsupportedReason,
	recordQueriesShadow,
	recordQueriesUncompared,
	recordQueriesUnsupported,
} from "./queriesMetrics"
import {entityRef} from "./recordKey"
import {
	AmbiguousRecordError,
	decodeProposalAction,
	decodeProposalStatus,
	decodeProposalVersion,
	decodeVoteRecord,
	PROPOSAL_ACTION_COLUMNS,
	PROPOSAL_VERSION_COLUMNS,
	PROPOSAL_VOTE_KEY_COLUMNS,
	proposalStatusRecordId,
} from "./records"
import type {SystemSpaceLookup} from "./shadow"

export type QueriesShadowConfig = {
	enabled: boolean
	/** Most proposals of one list response compared, from the front. */
	listMax: number
	/** Most voters one detail comparison reads; more is counted unsupported. */
	voterMax: number
	/** Most comparisons running at once; a new one past it is dropped (counted `busy`). */
	maxInFlight: number
}

const DEFAULTS = {listMax: 5, voterMax: 25, maxInFlight: 8}
/** Most backlinks one proposal (versions plus actions) is read with. */
const RECORD_MAX = 256
/** Most candidate proposals one active check reads. */
const CANDIDATE_MAX = 25
const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readQueriesShadowConfig(env: Record<string, string | undefined>): QueriesShadowConfig {
	const flag = env.GEODB_GOVERNANCE_QUERIES_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		listMax: wholeNumber(env.GEODB_GOVERNANCE_QUERIES_SHADOW_LIST_MAX, DEFAULTS.listMax, 100),
		voterMax: wholeNumber(env.GEODB_GOVERNANCE_QUERIES_SHADOW_VOTER_MAX, DEFAULTS.voterMax, 200),
		maxInFlight: wholeNumber(env.GEODB_GOVERNANCE_QUERIES_SHADOW_MAX_IN_FLIGHT, DEFAULTS.maxInFlight, 64),
	}
}

class Unsupported extends Error {
	constructor(readonly reason: QueryUnsupportedReason) {
		super(reason)
	}
}

export type QueriesLogger = {warn: (message: string, data?: Record<string, unknown>) => void}

export type ListFilters = {actionTypes?: readonly string[]; excludeActionTypes?: readonly string[]}

export type QueriesShadow = {
	detail(proposal: ProposalWithVotes): void
	list(proposals: readonly ProposalListItem[], filters: ListFilters): void
	/** `served` is what Postgres answered for the member or editor check. */
	active(kind: "member" | "editor", targetSpaceId: string, served: boolean): void
}

const MAX_LOGGED_MISMATCHES = 20
const ACTION_TYPE = {member: "AddMember", editor: "AddEditor"} as const

type GeoActions = {actions: ReturnType<typeof decodeProposalAction>[]; block: bigint}

export function createGovernanceQueriesShadow(deps: {
	config: QueriesShadowConfig
	geoDb: GeoDbCardClient
	systemSpaceOf: SystemSpaceLookup
	log: QueriesLogger
}): QueriesShadow {
	const {config, geoDb, systemSpaceOf, log} = deps
	activateQueriesMetrics()
	let inFlight = 0
	let logged = 0

	const space = async (family: "governance" | "votes"): Promise<string> => {
		const id = await systemSpaceOf(family)
		if (!id) throw new Unsupported("system_space_missing")
		return id
	}

	/** The actions of a proposal's latest version, found by the proposal's backlinks (versions and actions both point at it). */
	async function actionsAtLatestVersion(proposalRef: string): Promise<GeoActions> {
		const geoSpaceId = await space("governance")
		if (!geoDb.readBacklinks) throw new Unsupported("backlinks_unavailable")
		const links = await geoDb.readBacklinks({geoSpaceIds: [geoSpaceId], entityId: proposalRef, limit: RECORD_MAX})
		if (links.length >= RECORD_MAX) throw new Unsupported("too_many_records")
		const ids = (prop: string) => [...new Set(links.filter((l) => l.prop === prop).map((l) => l.from))]
		const versions = (
			await Promise.all(
				ids(PROPOSAL_VERSION_COLUMNS.proposal).map(async (id) =>
					decodeProposalVersion(await geoDb.readCard({geoSpaceId, entityId: id})),
				),
			)
		).filter((v): v is NonNullable<typeof v> => v !== null)
		if (versions.length === 0) throw new Unsupported("version_missing")
		const block = versions.reduce((best, v) => (v.block > best ? v.block : best), versions[0]?.block ?? 0n)
		const all = await Promise.all(
			ids(PROPOSAL_ACTION_COLUMNS.proposal).map(async (id) =>
				decodeProposalAction(await geoDb.readCard({geoSpaceId, entityId: id})),
			),
		)
		const actions = all
			.filter((a): a is NonNullable<typeof a> => a !== null && a.block === block)
			.sort((a, b) => (a.index < b.index ? -1 : a.index > b.index ? 1 : 0))
		return {actions, block}
	}

	const sameActions = (served: readonly ProposalAction[], geo: GeoActions["actions"]): boolean =>
		served.length === geo.length &&
		served.every((s, i) => {
			const g = geo[i]
			return (
				g !== null &&
				g !== undefined &&
				s.actionType === g.actionType &&
				(s.targetId === null ? g.target === null : g.target === entityRef(s.targetId)) &&
				(s.contentUri ?? null) === g.contentUri &&
				(s.contentId ?? null) === g.contentId
			)
		})

	/** Vote records of the proposal at `version`, by its backlinks in the votes Space. */
	async function votesAtVersion(p: ProposalWithVotes): Promise<{yes: bigint; no: bigint; abstain: bigint}> {
		const geoSpaceId = await space("votes")
		if (!geoDb.readBacklinks) throw new Unsupported("backlinks_unavailable")
		const limit = config.voterMax + 1
		const links = await geoDb.readBacklinks({geoSpaceIds: [geoSpaceId], entityId: entityRef(p.id), limit})
		const from = [...new Set(links.filter((l) => l.prop === PROPOSAL_VOTE_KEY_COLUMNS.proposal).map((l) => l.from))]
		if (from.length > config.voterMax || links.length >= limit) throw new Unsupported("too_many_records")
		const version = BigInt.asUintN(32, BigInt(p.proposalVersion))
		const tally = {yes: 0n, no: 0n, abstain: 0n}
		const records = await Promise.all(
			from.map(async (id) => decodeVoteRecord(await geoDb.readCard({geoSpaceId, entityId: id}))),
		)
		for (const r of records) {
			if (r === null || r.version !== version) continue
			if (r.option === 1) tally.yes++
			else if (r.option === 2) tally.no++
			else tally.abstain++
		}
		return tally
	}

	/**
	 * A ref from a backlink is the proposal's gaia id only when that id was v4 (refs of other ids are a one-way remap). For
	 * a legacy id the status record id below is wrong and reads empty, so the proposal stays a candidate: the bias is
	 * toward the upper bound, never toward a false mismatch.
	 */

	/**
	 * Whether a proposal not yet executed carries an action of `type` at `targetRef` in its latest version. It cannot say
	 * the proposal is still open: that needs vote tallies, editor counts and the space, which the records do not give
	 * this read. So geo-db true is an upper bound on Postgres true, and the comparison is one-sided (see `compareActive`).
	 */
	async function activeFromRecords(type: string, targetRef: string): Promise<boolean> {
		const geoSpaceId = await space("governance")
		if (!geoDb.readBacklinks) throw new Unsupported("backlinks_unavailable")
		const links = await geoDb.readBacklinks({geoSpaceIds: [geoSpaceId], entityId: targetRef, limit: RECORD_MAX})
		if (links.length >= RECORD_MAX) throw new Unsupported("too_many_records")
		const ids = [...new Set(links.filter((l) => l.prop === PROPOSAL_ACTION_COLUMNS.target).map((l) => l.from))]
		const candidates = (
			await Promise.all(
				ids.map(async (id) => decodeProposalAction(await geoDb.readCard({geoSpaceId, entityId: id}))),
			)
		).filter((a): a is NonNullable<typeof a> => a !== null && a.actionType === type)
		const proposals = [...new Set(candidates.map((a) => a.proposal))]
		if (proposals.length > CANDIDATE_MAX) throw new Unsupported("too_many_candidates")
		for (const ref of proposals) {
			const {block} = await actionsAtLatestVersion(ref)
			if (!candidates.some((a) => a.proposal === ref && a.block === block)) continue
			const status = decodeProposalStatus(
				await geoDb.readCard({geoSpaceId, entityId: proposalStatusRecordId(ref)}),
			)
			if (status.executedAt === null) return true
		}
		return false
	}

	function classify(query: Query, error: unknown) {
		if (error instanceof Unsupported) {
			recordQueriesShadow(query, "unsupported")
			recordQueriesUnsupported(query, error.reason)
		} else if (error instanceof AmbiguousRecordError) {
			recordQueriesShadow(query, "unsupported")
			recordQueriesUnsupported(query, "ambiguous_record")
		} else {
			recordQueriesShadow(query, "failed")
			if (!(error instanceof GeoDbError)) log.warn("governance queries shadow failed", {error: String(error)})
		}
	}

	function outcome(query: Query, match: boolean, detail: Record<string, unknown>) {
		recordQueriesShadow(query, match ? "match" : "mismatch")
		if (!match && logged < MAX_LOGGED_MISMATCHES) {
			logged++
			log.warn("governance queries shadow mismatch", {query, ...detail})
		}
	}

	async function compareDetail(p: ProposalWithVotes) {
		recordQueriesUncompared("detail", "action_settings_fields_from_postgres")
		try {
			const {actions} = await actionsAtLatestVersion(entityRef(p.id))
			const actionsMatch = sameActions(p.actions, actions)
			const tally = await votesAtVersion(p)
			const votesMatch = tally.yes === p.yesCount && tally.no === p.noCount && tally.abstain === p.abstainCount
			outcome("detail", actionsMatch && votesMatch, {actionsMatch, votesMatch, actionCount: p.actions.length})
		} catch (error) {
			classify("detail", error)
		}
	}

	async function compareListItem(p: ProposalListItem, filters: ListFilters) {
		recordQueriesUncompared("list", "list_membership_from_postgres")
		recordQueriesUncompared("list", "list_order_and_cursor_from_postgres")
		recordQueriesUncompared("list", "list_status_filter_from_postgres")
		recordQueriesUncompared("list", "list_vote_counts_from_postgres")
		recordQueriesUncompared("list", "action_settings_fields_from_postgres")
		try {
			const {actions} = await actionsAtLatestVersion(entityRef(p.id))
			const actionsMatch = sameActions(p.actions, actions)
			const types = new Set(actions.map((a) => a?.actionType))
			const include = filters.actionTypes ?? []
			const exclude = filters.excludeActionTypes ?? []
			const filterMatch =
				(include.length === 0 || include.some((t) => types.has(t))) && !exclude.some((t) => types.has(t))
			outcome("list", actionsMatch && filterMatch, {actionsMatch, filterMatch, actionCount: p.actions.length})
		} catch (error) {
			classify("list", error)
		}
	}

	async function compareActive(kind: "member" | "editor", targetSpaceId: string, served: boolean) {
		const query: Query = kind === "member" ? "active_member" : "active_editor"
		recordQueriesUncompared(query, "active_space_scope_from_postgres")
		recordQueriesUncompared(query, "active_vote_counts_from_postgres")
		try {
			const geoHas = await activeFromRecords(ACTION_TYPE[kind], entityRef(targetSpaceId))
			if (served && !geoHas) return outcome(query, false, {served, geoHas})
			if (!served && geoHas) {
				// A not-executed proposal with this action exists, but it may be rejected, expired or in another space.
				recordQueriesShadow(query, "unsupported")
				recordQueriesUnsupported(query, "status_inputs_not_read")
				return
			}
			outcome(query, true, {served, geoHas})
		} catch (error) {
			classify(query, error)
		}
	}

	function run(query: Query, work: () => Promise<void>) {
		if (!config.enabled) return
		if (inFlight >= config.maxInFlight) {
			recordQueriesShadow(query, "unsupported")
			recordQueriesUnsupported(query, "busy")
			return
		}
		inFlight++
		// Each compare catches everything it can throw; the .catch is the last line of defence for the response.
		void work()
			.catch(() => {})
			.finally(() => {
				inFlight--
			})
	}

	return {
		detail: (proposal) => run("detail", () => compareDetail(proposal)),
		list: (proposals, filters) => {
			for (const p of proposals.slice(0, config.listMax)) run("list", () => compareListItem(p, filters))
		},
		active: (kind, targetSpaceId, served) =>
			run(kind === "member" ? "active_member" : "active_editor", () =>
				compareActive(kind, targetSpaceId, served),
			),
	}
}
