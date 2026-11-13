import {beforeEach, describe, expect, it, vi} from "vitest"

import type {GeoCardRow} from "../../geodb/card"
import {type GeoDbCardClient, GeoDbError} from "../../geodb/client"
import {__resetServeGateForTests, renderServeGateMetrics} from "../../geodb/serveGate"
import {__resetGovernanceMetricsForTests} from "../governance/metrics"
import {entityRef} from "../governance/recordKey"
import {
	PROPOSAL_STATUS_COLUMNS,
	PROPOSAL_VERSION_COLUMNS,
	PROPOSAL_VOTE_COLUMNS,
	proposalStatusRecordId,
	proposalVoteRecordId,
} from "../governance/records"
import {createGovernanceServeWithoutShadow, createGovernanceShadow} from "../governance/shadow"
import {buildListItemResponse, buildProposalResponse} from "../router"
import {computeProposalStatus} from "../status"
import type {ProposalListItem, ProposalWithVotes} from "../types"

const NOW = 1_800_000_000n
const P = "11111111111111111111111111111111"
const V1 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
const V2 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
const GOV = "d".repeat(32)
const VOTES = "e".repeat(32)
const VER1 = "c1".repeat(16)

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

const row = (prop: string, raw: string): GeoCardRow => ({prop, raw})
const versionRows = (): GeoCardRow[] => {
	const v: Record<keyof typeof PROPOSAL_VERSION_COLUMNS, string> = {
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
	}
	return Object.entries(v).map(([k, raw]) =>
		row(PROPOSAL_VERSION_COLUMNS[k as keyof typeof PROPOSAL_VERSION_COLUMNS], raw),
	)
}
const votesOf = (p: ProposalWithVotes, option: string) =>
	new Map<string, GeoCardRow[]>(
		p.votes.map((v) => [
			proposalVoteRecordId(p.id, p.proposalVersion, v.voterId),
			[row(PROPOSAL_VOTE_COLUMNS.vote, option)],
		]),
	)

function client(
	records: Map<string, GeoCardRow[]>,
): GeoDbCardClient & {fail: {on: boolean; error: unknown}; hang: boolean} {
	const c = {
		fail: {on: false, error: new Error("boom") as unknown},
		hang: false,
		readCard: async ({entityId}: {entityId: string}) => {
			if (c.hang) return new Promise<GeoCardRow[]>(() => {})
			if (c.fail.on) throw c.fail.error
			return entityId === VER1 ? versionRows() : (records.get(entityId) ?? [])
		},
		readBacklinks: async () => [{prop: PROPOSAL_VERSION_COLUMNS.proposal, raw: "", from: VER1}],
	}
	return c as unknown as GeoDbCardClient & {fail: {on: boolean; error: unknown}; hang: boolean}
}

const spaces = async (family: "governance" | "votes") => (family === "governance" ? GOV : VOTES)
const settle = () => new Promise((r) => setTimeout(r, 0))
const cfg = {enabled: true, listMax: 2, voterMax: 3, maxInFlight: 8, escalations: false, rereadMs: 0}
const log = {warn: vi.fn()}
const make = (c: GeoDbCardClient, over: {ttl?: number; timeout?: number; now?: () => number} = {}) =>
	createGovernanceShadow({
		config: cfg,
		geoDb: c,
		systemSpaceOf: spaces,
		log,
		serve: {enabled: true, matchTtlMs: over.ttl ?? 60_000, timeoutMs: over.timeout ?? 500},
		...(over.now ? {now: over.now} : {}),
	})
const fallback = (reason: string) =>
	Number(
		new RegExp(
			`serve_fallback_total\\{family="governance",read="proposal_status",reason="${reason}"\\} (\\d+)`,
		).exec(renderServeGateMetrics())?.[1],
	)
const served = () =>
	Number(/serve_total\{family="governance",read="proposal_status"\} (\d+)/.exec(renderServeGateMetrics())?.[1])

beforeEach(() => {
	__resetGovernanceMetricsForTests()
	__resetServeGateForTests()
})

describe("governance serve", () => {
	it("serves geo-db's status once the shadow's comparison for the proposal matched", async () => {
		const p = proposal()
		const shadow = make(client(votesOf(p, "1")))
		shadow.detail(p, NOW)
		await settle()
		const got = await shadow.serveStatuses("detail", [p], NOW)
		expect(got.get(p.id)).toEqual(computeProposalStatus(p, NOW))
		expect(served()).toBe(1)
	})

	it("serves nothing while the switch is off, and exports no serve series", async () => {
		const p = proposal()
		const shadow = createGovernanceShadow({config: cfg, geoDb: client(votesOf(p, "1")), systemSpaceOf: spaces, log})
		shadow.detail(p, NOW)
		await settle()
		expect((await shadow.serveStatuses("detail", [p], NOW)).size).toBe(0)
		expect(renderServeGateMetrics()).toBe("")
	})

	it("falls back no_comparison for a proposal the shadow has not compared", async () => {
		const p = proposal()
		const shadow = make(client(votesOf(p, "1")))
		expect((await shadow.serveStatuses("detail", [p], NOW)).size).toBe(0)
		expect(fallback("no_comparison")).toBe(1)
	})

	it("falls back mismatch when geo-db disagreed, and does not trust a match from another surface", async () => {
		const p = proposal()
		const records = votesOf(p, "1")
		records.set(proposalStatusRecordId(P), [row(PROPOSAL_STATUS_COLUMNS.executedAt, `instant:${NOW}.000000`)])
		const shadow = make(client(records))
		shadow.detail(p, NOW)
		await settle()
		expect((await shadow.serveStatuses("detail", [p], NOW)).size).toBe(0)
		expect(fallback("mismatch")).toBe(1)
		expect((await shadow.serveStatuses("list", [p], NOW)).size).toBe(0)
		expect(fallback("no_comparison")).toBe(1)
	})

	it("falls back stale after the match TTL", async () => {
		let t = 0
		const p = proposal()
		const shadow = make(client(votesOf(p, "1")), {ttl: 100, now: () => t})
		shadow.detail(p, NOW)
		await settle()
		t = 101
		expect((await shadow.serveStatuses("detail", [p], NOW)).size).toBe(0)
		expect(fallback("stale")).toBe(1)
	})

	it("falls back unsupported when the comparison could not be made", async () => {
		const p = proposal()
		const c = client(votesOf(p, "1"))
		c.fail.on = true
		const shadow = make(c)
		shadow.detail(p, NOW)
		await settle()
		c.fail.on = false
		expect((await shadow.serveStatuses("detail", [p], NOW)).size).toBe(0)
		expect(fallback("unsupported")).toBe(1)
	})

	it("falls back error or timeout when the serving read fails or is slow, never rejecting", async () => {
		const p = proposal()
		const c = client(votesOf(p, "1"))
		const shadow = make(c, {timeout: 20})
		shadow.detail(p, NOW)
		await settle()
		c.fail.on = true
		expect((await shadow.serveStatuses("detail", [p], NOW)).size).toBe(0)
		expect(fallback("error")).toBe(1)
		c.fail.on = false
		c.hang = true
		expect((await shadow.serveStatuses("detail", [p], NOW)).size).toBe(0)
		expect(fallback("timeout")).toBe(1)
	})

	it("counts shadow_off for every proposal when the family has no shadow", async () => {
		const shadow = createGovernanceServeWithoutShadow()
		expect((await shadow.serveStatuses("list", [proposal(), proposal({id: V1})], NOW)).size).toBe(0)
		expect(fallback("shadow_off")).toBe(2)
	})
})

describe("response shape with a served status", () => {
	const p = proposal()
	const item: ProposalListItem = {
		...p,
		userVote: "YES",
	} as unknown as ProposalListItem
	const served = computeProposalStatus(p, NOW)

	it("is identical to Postgres's when geo-db's status equals it", () => {
		expect(buildProposalResponse(p, NOW, V1, served)).toEqual(buildProposalResponse(p, NOW, V1))
		expect(buildListItemResponse(item, NOW, served)).toEqual(buildListItemResponse(item, NOW))
		expect(JSON.stringify(buildProposalResponse(p, NOW, V1, served))).toBe(
			JSON.stringify(buildProposalResponse(p, NOW, V1)),
		)
	})

	it("keeps the shape and the Postgres-sourced fields when geo-db's status differs", () => {
		const other = {...served, status: "ACCEPTED" as const}
		const a = buildProposalResponse(p, NOW, V1)
		const b = buildProposalResponse(p, NOW, V1, other)
		expect(Object.keys(b)).toEqual(Object.keys(a))
		expect(b.status).toBe("ACCEPTED")
		expect(b.votes).toEqual(a.votes)
		expect(b.timing).toEqual(a.timing)
	})
})
