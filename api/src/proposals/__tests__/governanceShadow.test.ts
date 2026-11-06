import {existsSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"

import {beforeEach, describe, expect, it, vi} from "vitest"

import type {GeoCardRow} from "../../geodb/card"
import {type GeoDbCardClient, GeoDbError} from "../../geodb/client"
import {__resetGovernanceMetricsForTests, renderGovernanceMetrics} from "../governance/metrics"
import {entityRef, recordEntityId} from "../governance/recordKey"
import {
	decodeProposalStatus,
	decodeProposalVersion,
	decodeProposalVote,
	PROPOSAL_STATUS_COLUMNS,
	PROPOSAL_STATUS_KIND,
	PROPOSAL_VERSION_COLUMNS,
	PROPOSAL_VERSION_KIND,
	PROPOSAL_VOTE_COLUMNS,
	PROPOSAL_VOTE_KIND,
	proposalStatusRecordId,
	proposalVoteRecordId,
} from "../governance/records"
import {createGovernanceShadow, readGovernanceShadowConfig} from "../governance/shadow"
import {sameResult, statusFromRecords, tallyVotes} from "../governance/status"
import {parseSystemSpaces} from "../governance/systemSpaces"
import {computeProposalStatus} from "../status"
import type {ProposalListItem, ProposalWithVotes} from "../types"

const here = dirname(fileURLToPath(import.meta.url))
const NOW = 1_800_000_000n
const P = "11111111111111111111111111111111"
const V1 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
const V2 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

function proposal(over: Partial<ProposalWithVotes> = {}): ProposalWithVotes {
	return {
		id: P,
		spaceId: "22222222222222222222222222222222",
		name: null,
		proposedBy: V1,
		proposalVersion: 1,
		votingMode: "Fast",
		startTime: NOW - 3600n,
		endTime: NOW + 3600n,
		quorum: 1n,
		threshold: 2n,
		flatSupportThreshold: 2n,
		partialPercentageSupportThreshold: 0n,
		universalPercentageSupportThreshold: 0n,
		executeBy: NOW + 86_400n,
		totalEditors: 3n,
		executedAt: null,
		unexecutableAt: null,
		yesCount: 2n,
		noCount: 0n,
		abstainCount: 0n,
		votes: [
			{voterId: V1, vote: "YES"},
			{voterId: V2, vote: "YES"},
		],
		actions: [],
		...over,
	}
}

const inst = (s: bigint | number) => `instant:${s}.000000`
const row = (prop: string, raw: string): GeoCardRow => ({prop, raw})

describe("record ids", () => {
	it("hold the copied pins to the feeder's recordKinds", () => {
		const file = join(here, "../../../../geodb-feeder/src/recordKinds.ts")
		if (!existsSync(file)) return
		const text = readFileSync(file, "utf8")
		expect(text).toContain(PROPOSAL_VOTE_KIND)
		expect(text).toContain(PROPOSAL_VOTE_COLUMNS.vote)
		// proposalStatus is pinned by the PROPOSAL-STATUS change; once the feeder carries it the ids must agree.
		expect(text).toContain(PROPOSAL_VERSION_KIND)
		for (const id of Object.values(PROPOSAL_VERSION_COLUMNS)) expect(text).toContain(id)
		if (text.includes("proposalStatus:")) {
			expect(text).toContain(PROPOSAL_STATUS_KIND)
			expect(text).toContain(PROPOSAL_STATUS_COLUMNS.executedAt)
			expect(text).toContain(PROPOSAL_STATUS_COLUMNS.unexecutableAt)
		}
	})

	it("derives a v4-shaped id, stable, and distinct per key", () => {
		const a = proposalStatusRecordId(P)
		expect(a).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
		expect(proposalStatusRecordId(P)).toBe(a)
		expect(proposalStatusRecordId(V1)).not.toBe(a)
		expect(proposalVoteRecordId(P, 1, V1)).not.toBe(proposalVoteRecordId(P, 2, V1))
		expect(proposalVoteRecordId(P, 1, V1)).not.toBe(proposalVoteRecordId(P, 1, V2))
	})

	it("matches the feeder's golden vectors for id and int parts", () => {
		const file = join(here, "../../../../geodb-feeder/src/__tests__/vectors/record-key.json")
		if (!existsSync(file)) return
		type Part = {id: string} | {bytes: string} | {int: string}
		const {cases} = JSON.parse(readFileSync(file, "utf8")) as {
			cases: {kind: string; role: string; parts: Part[]; id: string}[]
		}
		const covered = cases.filter((c) => c.role === "entity" && c.parts.every((p) => "id" in p || "int" in p))
		expect(covered.length).toBeGreaterThan(0)
		for (const c of covered)
			expect(
				recordEntityId(
					c.kind,
					c.parts.map((p) => ("id" in p ? {id: p.id} : {int: BigInt((p as {int: string}).int)})),
				),
			).toBe(c.id.replaceAll("-", ""))
	})
})

const versionRows = (over: Partial<Record<keyof typeof PROPOSAL_VERSION_COLUMNS, string>> = {}): GeoCardRow[] => {
	const v: Record<keyof typeof PROPOSAL_VERSION_COLUMNS, string | undefined> = {
		proposal: entityRef(P),
		block: "100",
		votingMode: "Fast",
		startTime: String(NOW - 3600n),
		endTime: String(NOW + 3600n),
		quorum: "1",
		partial: "0",
		universal: "0",
		flat: "2",
		executeBy: String(NOW + 86_400n),
		...over,
	}
	return Object.entries(v)
		.filter(([, raw]) => raw !== undefined)
		.map(([k, raw]) => row(PROPOSAL_VERSION_COLUMNS[k as keyof typeof PROPOSAL_VERSION_COLUMNS], raw as string))
}

describe("decoders", () => {
	it("decodes a version record, deriving threshold from the mode and no deadline from no column", () => {
		const fast = decodeProposalVersion(versionRows())
		expect(fast?.block).toBe(100n)
		expect(fast?.snapshot.threshold).toBe(2n)
		expect(fast?.snapshot.executeBy).toBe(NOW + 86_400n)
		const slow = decodeProposalVersion(
			versionRows({votingMode: "Slow", partial: "51", flat: "0", executeBy: undefined}),
		)
		expect(slow?.snapshot.threshold).toBe(51n)
		expect(slow?.snapshot.executeBy).toBeNull()
		expect(decodeProposalVersion([])).toBeNull()
		expect(() => decodeProposalVersion(versionRows({quorum: undefined}))).toThrow()
		expect(() => decodeProposalVersion(versionRows({votingMode: "Medium"}))).toThrow()
	})

	it("reads executed_at and unexecutable_at as whole seconds, absent as null", () => {
		expect(decodeProposalStatus([])).toEqual({executedAt: null, unexecutableAt: null})
		expect(
			decodeProposalStatus([
				row(PROPOSAL_STATUS_COLUMNS.executedAt, "instant:1799999999.000000"),
				row(PROPOSAL_STATUS_COLUMNS.unexecutableAt, inst(5)),
			]),
		).toEqual({executedAt: 1_799_999_999n, unexecutableAt: 5n})
	})
	it("refuses two values for one column", () => {
		expect(() =>
			decodeProposalStatus([
				row(PROPOSAL_STATUS_COLUMNS.executedAt, inst(1)),
				row(PROPOSAL_STATUS_COLUMNS.executedAt, inst(2)),
			]),
		).toThrow()
	})
	it("reads a vote option, null for no record, and refuses an unknown option", () => {
		expect(decodeProposalVote([])).toBeNull()
		expect(decodeProposalVote([row(PROPOSAL_VOTE_COLUMNS.vote, "2")])).toBe(2)
		expect(decodeProposalVote([row(PROPOSAL_VOTE_COLUMNS.vote, "1.0")])).toBe(1)
		expect(() => decodeProposalVote([row(PROPOSAL_VOTE_COLUMNS.vote, "9")])).toThrow()
	})
})

describe("statusFromRecords", () => {
	it("equals computeProposalStatus when the records equal the columns", () => {
		const p = proposal()
		const got = statusFromRecords(p, {executedAt: null, unexecutableAt: null, yes: 2n, no: 0n, abstain: 0n}, NOW)
		expect(sameResult(got, computeProposalStatus(p, NOW))).toBe(true)
		expect(got.status).toBe("EXECUTABLE")
	})
	it("is ACCEPTED once an executed_at record exists, whatever the votes", () => {
		const got = statusFromRecords(
			proposal(),
			{executedAt: NOW, unexecutableAt: null, yes: 0n, no: 0n, abstain: 0n},
			NOW,
		)
		expect(got.status).toBe("ACCEPTED")
	})
	it("is REJECTED when unexecutable_at is set", () => {
		const got = statusFromRecords(
			proposal(),
			{executedAt: null, unexecutableAt: NOW, yes: 2n, no: 0n, abstain: 0n},
			NOW,
		)
		expect(got.status).toBe("REJECTED")
	})
	it("moves with the clock alone: executable, then rejected past execute_by", () => {
		const p = proposal()
		const r = {executedAt: null, unexecutableAt: null, yes: 2n, no: 0n, abstain: 0n}
		expect(statusFromRecords(p, r, NOW).status).toBe("EXECUTABLE")
		expect(statusFromRecords(p, r, NOW + 200_000n).status).toBe("REJECTED")
	})
	it("tallies options", () => {
		expect(tallyVotes([1, 1, 2, 3, 3, 3])).toEqual({yes: 2n, no: 1n, abstain: 3n})
	})
})

describe("config", () => {
	it("is off unless explicitly on", () => {
		expect(readGovernanceShadowConfig({}).enabled).toBe(false)
		expect(readGovernanceShadowConfig({GEODB_GOVERNANCE_SHADOW: "0"}).enabled).toBe(false)
		expect(readGovernanceShadowConfig({GEODB_GOVERNANCE_SHADOW: "1"}).enabled).toBe(true)
		expect(
			readGovernanceShadowConfig({GEODB_GOVERNANCE_SHADOW: "on", GEODB_GOVERNANCE_SHADOW_LIST_MAX: "9"}).listMax,
		).toBe(9)
		expect(
			readGovernanceShadowConfig({GEODB_GOVERNANCE_SHADOW: "on", GEODB_GOVERNANCE_SHADOW_LIST_MAX: "x"}).listMax,
		).toBe(5)
	})
	it("parses system-spaces.json, skipping malformed entries", () => {
		const geo = "c".repeat(32)
		const map = parseSystemSpaces(JSON.stringify({governance: {geo, nonce: "n"}, votes: {geo: "zz"}}))
		expect(map.get("governance")).toBe(geo)
		expect(map.has("votes")).toBe(false)
		expect(() => parseSystemSpaces("[]")).toThrow()
	})
})

describe("shadow", () => {
	const GOV = "d".repeat(32)
	const VOTES = "e".repeat(32)
	const settle = () => new Promise((r) => setTimeout(r, 0))
	const cfg = {enabled: true, listMax: 2, voterMax: 3, maxInFlight: 8}
	const log = {warn: vi.fn()}

	const VER1 = "c1".repeat(16)
	const VER2 = "c2".repeat(16)
	/** A client whose proposals each have the given version records (default: one matching `proposal()`). */
	function client(
		records: Map<string, GeoCardRow[]>,
		versions: Record<string, GeoCardRow[]> = {[VER1]: versionRows()},
	): GeoDbCardClient & {reads: string[]} {
		const reads: string[] = []
		return {
			reads,
			readCard: async ({entityId}) => {
				reads.push(entityId)
				return versions[entityId] ?? records.get(entityId) ?? []
			},
			readBacklinks: async () =>
				Object.keys(versions).map((from) => ({prop: PROPOSAL_VERSION_COLUMNS.proposal, raw: "", from})),
		}
	}
	const spaces = async (family: "governance" | "votes") => (family === "governance" ? GOV : VOTES)
	const metric = (text: string, line: string) =>
		Number(new RegExp(`^${line.replace(/[{}]/g, "\\$&")} (\\d+)$`, "m").exec(text)?.[1])
	const votesOf = (p: ProposalWithVotes, option: string) =>
		new Map(
			p.votes.map((v) => [
				proposalVoteRecordId(p.id, p.proposalVersion, v.voterId),
				[row(PROPOSAL_VOTE_COLUMNS.vote, option)],
			]),
		)

	beforeEach(() => {
		__resetGovernanceMetricsForTests()
		log.warn.mockClear()
	})

	it("exports nothing until a shadow is mounted", () => {
		expect(renderGovernanceMetrics()).toBe("")
	})

	it("counts a detail match when the records agree, and exports every series at zero", async () => {
		const p = proposal()
		const c = client(votesOf(p, "1"))
		createGovernanceShadow({config: cfg, geoDb: c, systemSpaceOf: spaces, log}).detail(p, NOW)
		await settle()
		const text = renderGovernanceMetrics()
		expect(metric(text, 'gaia_api_governance_shadow_total{surface="detail",result="match"}')).toBe(1)
		expect(metric(text, 'gaia_api_governance_shadow_total{surface="list",result="mismatch"}')).toBe(0)
		expect(
			metric(
				text,
				'gaia_api_governance_shadow_uncompared_total{surface="detail",input="total_editors_from_postgres"}',
			),
		).toBe(1)
		expect(text).not.toContain("snapshot_from_postgres")
		expect(c.reads).toContain(proposalStatusRecordId(P))
	})

	it("counts a mismatch when geo-db has executed_at that Postgres lacks", async () => {
		const p = proposal()
		const records = votesOf(p, "1")
		records.set(proposalStatusRecordId(P), [row(PROPOSAL_STATUS_COLUMNS.executedAt, inst(NOW))])
		createGovernanceShadow({config: cfg, geoDb: client(records), systemSpaceOf: spaces, log}).detail(p, NOW)
		await settle()
		expect(
			metric(renderGovernanceMetrics(), 'gaia_api_governance_shadow_total{surface="detail",result="mismatch"}'),
		).toBe(1)
		expect(log.warn).toHaveBeenCalledTimes(1)
	})

	const unsupportedCount = (reason: string) =>
		metric(
			renderGovernanceMetrics(),
			`gaia_api_governance_shadow_unsupported_total{surface="detail",reason="${reason}"}`,
		)

	it("takes the snapshot from the version with the highest block, not the first listed", async () => {
		const p = proposal()
		// the older version would make the proposal Rejected-looking (quorum 99 unmet); the later one matches Postgres
		const versions = {
			[VER2]: versionRows({block: "200"}),
			[VER1]: versionRows({block: "100", quorum: "99"}),
		}
		createGovernanceShadow({
			config: cfg,
			geoDb: client(votesOf(p, "1"), versions),
			systemSpaceOf: spaces,
			log,
		}).detail(p, NOW)
		await settle()
		expect(
			metric(renderGovernanceMetrics(), 'gaia_api_governance_shadow_total{surface="detail",result="match"}'),
		).toBe(1)
	})

	it("counts a settings difference as a mismatch", async () => {
		const p = proposal()
		const versions = {[VER1]: versionRows({quorum: "99"})}
		createGovernanceShadow({
			config: cfg,
			geoDb: client(votesOf(p, "1"), versions),
			systemSpaceOf: spaces,
			log,
		}).detail(p, NOW)
		await settle()
		expect(
			metric(renderGovernanceMetrics(), 'gaia_api_governance_shadow_total{surface="detail",result="mismatch"}'),
		).toBe(1)
	})

	it("counts geo Fast against Postgres Slow as escalation_pending, not a mismatch", async () => {
		const p = proposal({
			votingMode: "Slow",
			threshold: 51n,
			partialPercentageSupportThreshold: 51n,
			flatSupportThreshold: 0n,
		})
		createGovernanceShadow({config: cfg, geoDb: client(votesOf(p, "1")), systemSpaceOf: spaces, log}).detail(p, NOW)
		await settle()
		const text = renderGovernanceMetrics()
		expect(metric(text, 'gaia_api_governance_shadow_total{surface="detail",result="mismatch"}')).toBe(0)
		expect(metric(text, 'gaia_api_governance_shadow_total{surface="detail",result="unsupported"}')).toBe(1)
		expect(unsupportedCount("escalation_pending")).toBe(1)
	})

	it("is unsupported with no version record, or a client that cannot read backlinks", async () => {
		const p = proposal()
		createGovernanceShadow({config: cfg, geoDb: client(votesOf(p, "1"), {}), systemSpaceOf: spaces, log}).detail(
			p,
			NOW,
		)
		const noBacklinks: GeoDbCardClient = {readCard: async () => []}
		createGovernanceShadow({config: cfg, geoDb: noBacklinks, systemSpaceOf: spaces, log}).detail(p, NOW)
		await settle()
		expect(unsupportedCount("version_missing")).toBe(1)
		expect(unsupportedCount("backlinks_unavailable")).toBe(1)
	})

	it("counts a mismatch when a voter's record is missing", async () => {
		const p = proposal()
		const records = votesOf(p, "1")
		records.delete(proposalVoteRecordId(P, 1, V2))
		createGovernanceShadow({config: cfg, geoDb: client(records), systemSpaceOf: spaces, log}).detail(p, NOW)
		await settle()
		expect(
			metric(renderGovernanceMetrics(), 'gaia_api_governance_shadow_total{surface="detail",result="mismatch"}'),
		).toBe(1)
	})

	it("counts a mismatch when a voter's option differs", async () => {
		const p = proposal()
		createGovernanceShadow({config: cfg, geoDb: client(votesOf(p, "2")), systemSpaceOf: spaces, log}).detail(p, NOW)
		await settle()
		expect(
			metric(renderGovernanceMetrics(), 'gaia_api_governance_shadow_total{surface="detail",result="mismatch"}'),
		).toBe(1)
	})

	it("is unsupported with too many voters, reading no vote record", async () => {
		const p = proposal({votes: ["a", "b", "c", "d"].map((x) => ({voterId: x.repeat(32), vote: "YES" as const}))})
		const c = client(new Map())
		createGovernanceShadow({config: cfg, geoDb: c, systemSpaceOf: spaces, log}).detail(p, NOW)
		await settle()
		const text = renderGovernanceMetrics()
		expect(metric(text, 'gaia_api_governance_shadow_total{surface="detail",result="unsupported"}')).toBe(1)
		expect(
			metric(text, 'gaia_api_governance_shadow_unsupported_total{surface="detail",reason="too_many_voters"}'),
		).toBe(1)
		expect(c.reads).toEqual([VER1, proposalStatusRecordId(P)])
	})

	it("is unsupported when the system Space is not founded", async () => {
		createGovernanceShadow({config: cfg, geoDb: client(new Map()), systemSpaceOf: async () => null, log}).detail(
			proposal(),
			NOW,
		)
		await settle()
		expect(
			metric(
				renderGovernanceMetrics(),
				'gaia_api_governance_shadow_unsupported_total{surface="detail",reason="system_space_missing"}',
			),
		).toBe(1)
	})

	it("is unsupported on an ambiguous record", async () => {
		const records = new Map([
			[
				proposalStatusRecordId(P),
				[row(PROPOSAL_STATUS_COLUMNS.executedAt, inst(1)), row(PROPOSAL_STATUS_COLUMNS.executedAt, inst(2))],
			],
		])
		createGovernanceShadow({config: cfg, geoDb: client(records), systemSpaceOf: spaces, log}).detail(
			proposal(),
			NOW,
		)
		await settle()
		expect(
			metric(
				renderGovernanceMetrics(),
				'gaia_api_governance_shadow_unsupported_total{surface="detail",reason="ambiguous_record"}',
			),
		).toBe(1)
	})

	it("counts failed when geo-db errors, and never throws", async () => {
		const geoDb: GeoDbCardClient = {
			readCard: async () => {
				throw new GeoDbError("timeout", "slow")
			},
			readBacklinks: async () => {
				throw new GeoDbError("timeout", "slow")
			},
		}
		expect(() =>
			createGovernanceShadow({config: cfg, geoDb, systemSpaceOf: spaces, log}).detail(proposal(), NOW),
		).not.toThrow()
		await settle()
		expect(
			metric(renderGovernanceMetrics(), 'gaia_api_governance_shadow_total{surface="detail",result="failed"}'),
		).toBe(1)
	})

	it("compares only the first listMax proposals of a list, taking votes from Postgres", async () => {
		const items: ProposalListItem[] = ["1", "2", "3"].map((n) => {
			const {votes: _v, ...rest} = proposal({id: n.repeat(32)})
			return {...rest, userVote: null}
		})
		const c = client(new Map())
		createGovernanceShadow({config: cfg, geoDb: c, systemSpaceOf: spaces, log}).list(items, NOW)
		await settle()
		const text = renderGovernanceMetrics()
		expect(c.reads.filter((r) => r !== VER1)).toHaveLength(2)
		expect(metric(text, 'gaia_api_governance_shadow_total{surface="list",result="match"}')).toBe(2)
		expect(
			metric(text, 'gaia_api_governance_shadow_uncompared_total{surface="list",input="votes_from_postgres"}'),
		).toBe(2)
	})

	it("drops a comparison past maxInFlight, counted busy", async () => {
		let release: () => void = () => {}
		const gate = new Promise<void>((r) => {
			release = r
		})
		const geoDb: GeoDbCardClient = {
			readCard: async () => {
				await gate
				return []
			},
		}
		const s = createGovernanceShadow({config: {...cfg, maxInFlight: 1}, geoDb, systemSpaceOf: spaces, log})
		s.list([proposal({id: "1".repeat(32)}), proposal({id: "2".repeat(32)})] as unknown as ProposalListItem[], NOW)
		await settle()
		expect(
			metric(
				renderGovernanceMetrics(),
				'gaia_api_governance_shadow_unsupported_total{surface="list",reason="busy"}',
			),
		).toBe(1)
		release()
		await settle()
	})

	it("does nothing when disabled", async () => {
		const c = client(new Map())
		createGovernanceShadow({config: {...cfg, enabled: false}, geoDb: c, systemSpaceOf: spaces, log}).detail(
			proposal(),
			NOW,
		)
		await settle()
		expect(c.reads).toHaveLength(0)
	})
})
