/**
 * The governance shadow (GEODB_GOVERNANCE_SHADOW, off by default; flag-gated like the gateway's shadows).
 *
 * When the api serves a proposal or a list of proposals from Postgres it also reads the keyed governance records from
 * geo-db, computes the status from them (`statusFromRecords`) and counts whether it equals the status Postgres served.
 * It runs after the response is built, in the background, bounded by `maxInFlight`, and swallows every error: it
 * never changes, delays or fails a response.
 *
 * What geo-db supplies: the proposal's status record (executed_at, unexecutable_at) and, on the detail surface, the
 * vote record of each voter Postgres lists for the current version, and the settings snapshot, read from the
 * proposal's latest proposalVersion record (highest block, found by backlinks). A geo Fast version against a Postgres
 * Slow one is a fast-to-slow escalation geo-db does not carry yet; it counts unsupported{escalation_pending}, not a
 * mismatch. What Postgres still supplies (counted in `gaia_api_governance_shadow_uncompared_total`): the editor count, and on the list surface
 * the vote counts (a list carries no voter ids, and the records are keyed by voter). Proposal actions and Space voting
 * settings are not status inputs and are not read: a proposal keeps the settings it was created under, and status
 * never looks at its actions.
 */
import {type GeoDbCardClient, GeoDbError} from "../../geodb/client"
import {computeProposalStatus} from "../status"
import type {ProposalListItem, ProposalWithVotes, StatusComputationResult} from "../types"
import {
	activateGovernanceMetrics,
	recordGovernanceShadow,
	recordGovernanceUncompared,
	recordGovernanceUnsupported,
	type Surface,
	type UnsupportedReason,
} from "./metrics"
import {entityRef} from "./recordKey"
import {
	AmbiguousRecordError,
	decodeProposalStatus,
	decodeProposalVersion,
	decodeProposalVote,
	PROPOSAL_VERSION_COLUMNS,
	proposalStatusRecordId,
	proposalVoteRecordId,
} from "./records"
import {type StatusSnapshot, sameResult, statusFromRecords, tallyVotes} from "./status"

export type GovernanceShadowConfig = {
	enabled: boolean
	/** Most proposals of one list response compared, from the front. */
	listMax: number
	/** Most voters one detail comparison reads; more is counted unsupported. */
	voterMax: number
	/** Most comparisons running at once; a new one past it is dropped (counted `busy`). */
	maxInFlight: number
}

const DEFAULTS = {listMax: 5, voterMax: 25, maxInFlight: 8}
/** Most version records one proposal is read with; a proposal has one per settings-bearing event, a handful at most. */
const VERSION_MAX = 64
const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readGovernanceShadowConfig(env: Record<string, string | undefined>): GovernanceShadowConfig {
	const flag = env.GEODB_GOVERNANCE_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		listMax: wholeNumber(env.GEODB_GOVERNANCE_SHADOW_LIST_MAX, DEFAULTS.listMax, 100),
		voterMax: wholeNumber(env.GEODB_GOVERNANCE_SHADOW_VOTER_MAX, DEFAULTS.voterMax, 200),
		maxInFlight: wholeNumber(env.GEODB_GOVERNANCE_SHADOW_MAX_IN_FLIGHT, DEFAULTS.maxInFlight, 64),
	}
}

/** The geo-db system Spaces the records live in (`system-spaces.json`, beside the space map), or null where not founded. */
export type SystemSpaceLookup = (family: "governance" | "votes") => Promise<string | null>

class Unsupported extends Error {
	constructor(readonly reason: UnsupportedReason) {
		super(reason)
	}
}

export type GovernanceLogger = {warn: (message: string, data?: Record<string, unknown>) => void}

export type GovernanceShadow = {
	detail(proposal: ProposalWithVotes, nowSeconds: bigint): void
	list(proposals: readonly ProposalListItem[], nowSeconds: bigint): void
}

const OPTION: Record<string, 1 | 2 | 3> = {YES: 1, NO: 2, ABSTAIN: 3}
const MAX_LOGGED_MISMATCHES = 20

export function createGovernanceShadow(deps: {
	config: GovernanceShadowConfig
	geoDb: GeoDbCardClient
	systemSpaceOf: SystemSpaceLookup
	log: GovernanceLogger
}): GovernanceShadow {
	const {config, geoDb, systemSpaceOf, log} = deps
	activateGovernanceMetrics()
	let inFlight = 0
	let logged = 0

	const readRecord = async (family: "governance" | "votes", entityId: string) => {
		const geoSpaceId = await systemSpaceOf(family)
		if (!geoSpaceId) throw new Unsupported("system_space_missing")
		return geoDb.readCard({geoSpaceId, entityId})
	}

	/**
	 * The proposal's settings from its latest version record (highest block). A Fast version against Postgres's Slow is
	 * an escalation (PROPOSAL_SETTINGS_UPDATED rewrites the version in place in Postgres) geo-db does not carry yet.
	 */
	async function snapshotFromVersions(p: ProposalWithVotes | ProposalListItem): Promise<StatusSnapshot> {
		const geoSpaceId = await systemSpaceOf("governance")
		if (!geoSpaceId) throw new Unsupported("system_space_missing")
		if (!geoDb.readBacklinks) throw new Unsupported("backlinks_unavailable")
		const links = await geoDb.readBacklinks({
			geoSpaceIds: [geoSpaceId],
			entityId: entityRef(p.id),
			limit: VERSION_MAX,
		})
		const from = [...new Set(links.filter((l) => l.prop === PROPOSAL_VERSION_COLUMNS.proposal).map((l) => l.from))]
		if (from.length === 0) throw new Unsupported("version_missing")
		if (links.length >= VERSION_MAX) throw new Unsupported("too_many_versions")
		const versions = (
			await Promise.all(
				from.map(async (id) => decodeProposalVersion(await geoDb.readCard({geoSpaceId, entityId: id}))),
			)
		).filter((v): v is NonNullable<typeof v> => v !== null)
		const latest = versions.reduce<(typeof versions)[number] | null>(
			(best, v) => (best === null || v.block > best.block ? v : best),
			null,
		)
		if (!latest) throw new Unsupported("version_missing")
		if (latest.snapshot.votingMode === "Fast" && p.votingMode === "Slow")
			throw new Unsupported("escalation_pending")
		return {...latest.snapshot, totalEditors: p.totalEditors}
	}

	/** Status from geo-db for one proposal, plus which inputs it took from Postgres. */
	async function fromGeoDb(surface: Surface, p: ProposalWithVotes | ProposalListItem, nowSeconds: bigint) {
		const snapshot = await snapshotFromVersions(p)
		const status = decodeProposalStatus(await readRecord("governance", proposalStatusRecordId(p.id)))
		let tally = {yes: p.yesCount, no: p.noCount, abstain: p.abstainCount}
		let voteMismatch = false
		if (surface === "detail" && "votes" in p) {
			if (p.votes.length > config.voterMax) throw new Unsupported("too_many_voters")
			const options = await Promise.all(
				p.votes.map(async (v) =>
					decodeProposalVote(
						await readRecord("votes", proposalVoteRecordId(p.id, p.proposalVersion, v.voterId)),
					),
				),
			)
			voteMismatch = options.some((o, i) => o === null || o !== OPTION[p.votes[i]?.vote ?? ""])
			tally = tallyVotes(options.filter((o): o is 1 | 2 | 3 => o !== null))
		}
		return {result: statusFromRecords(snapshot, {...status, ...tally}, nowSeconds), voteMismatch}
	}

	async function compare(surface: Surface, p: ProposalWithVotes | ProposalListItem, nowSeconds: bigint) {
		const served: StatusComputationResult = computeProposalStatus(p, nowSeconds)
		recordGovernanceUncompared(surface, "total_editors_from_postgres")
		if (surface === "list") recordGovernanceUncompared(surface, "votes_from_postgres")
		try {
			const {result, voteMismatch} = await fromGeoDb(surface, p, nowSeconds)
			const match = sameResult(served, result) && !voteMismatch
			recordGovernanceShadow(surface, match ? "match" : "mismatch")
			if (!match && logged < MAX_LOGGED_MISMATCHES) {
				logged++
				log.warn("governance shadow mismatch", {
					proposalId: p.id,
					surface,
					served: served.status,
					geoDb: result.status,
					voteMismatch,
				})
			}
		} catch (error) {
			if (error instanceof Unsupported) {
				recordGovernanceShadow(surface, "unsupported")
				recordGovernanceUnsupported(surface, error.reason)
			} else if (error instanceof AmbiguousRecordError) {
				recordGovernanceShadow(surface, "unsupported")
				recordGovernanceUnsupported(surface, "ambiguous_record")
			} else {
				recordGovernanceShadow(surface, "failed")
				if (!(error instanceof GeoDbError)) log.warn("governance shadow failed", {error: String(error)})
			}
		}
	}

	function run(surface: Surface, proposals: readonly (ProposalWithVotes | ProposalListItem)[], nowSeconds: bigint) {
		if (!config.enabled) return
		for (const p of proposals) {
			if (inFlight >= config.maxInFlight) {
				recordGovernanceShadow(surface, "unsupported")
				recordGovernanceUnsupported(surface, "busy")
				continue
			}
			inFlight++
			// compare() catches everything it can throw; the .catch is the last line of defence for the response.
			void compare(surface, p, nowSeconds)
				.catch(() => {})
				.finally(() => {
					inFlight--
				})
		}
	}

	return {
		detail: (proposal, nowSeconds) => run("detail", [proposal], nowSeconds),
		list: (proposals, nowSeconds) => run("list", proposals.slice(0, config.listMax), nowSeconds),
	}
}
