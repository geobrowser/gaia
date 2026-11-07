import {beforeEach, describe, expect, it, vi} from "vitest"

import type {GeoBacklinkRow, GeoCardRow} from "../../geodb/card"
import type {GeoDbCardClient} from "../../geodb/client"
import {__resetQueriesMetricsForTests, renderQueriesMetrics} from "../governance/queriesMetrics"
import {createGovernanceQueriesShadow, readQueriesShadowConfig} from "../governance/queriesShadow"
import {entityRef} from "../governance/recordKey"
import {
	PROPOSAL_ACTION_COLUMNS as A,
	PROPOSAL_STATUS_COLUMNS,
	PROPOSAL_VOTE_COLUMNS,
	PROPOSAL_VOTE_KEY_COLUMNS,
	proposalStatusRecordId,
	PROPOSAL_VERSION_COLUMNS as V,
} from "../governance/records"
import type {ProposalAction, ProposalListItem, ProposalWithVotes} from "../types"

const P = "11111111111111111111111111111111"
// A v4-shaped id, so its geo-db ref is itself.
const P4 = "11111111111141111111111111111111".replace(/^(.{12})4(.{3})1/, "$14$28")
const TARGET = "33333333333343333333333333333333"
const GOV = "a0".repeat(16)
const VOTES = "b0".repeat(16)
const NOW = 1_800_000_000n

const row = (prop: string, raw: string): GeoCardRow => ({prop, raw})
const versionRows = (block: number): GeoCardRow[] => [
	row(V.votingMode, "Fast"),
	row(V.block, String(block)),
	row(V.startTime, "1"),
	row(V.endTime, "2"),
	row(V.quorum, "1"),
	row(V.partial, "0"),
	row(V.universal, "0"),
	row(V.flat, "2"),
]
const actionRows = (proposalRef: string, block: number, index: number, type: string, target?: string) => [
	row(A.proposal, proposalRef),
	row(A.block, String(block)),
	row(A.index, String(index)),
	row(A.actionType, type),
	...(target ? [row(A.target, entityRef(target))] : []),
]
const servedAction = (over: Partial<ProposalAction>): ProposalAction => ({
	actionType: "AddMember",
	targetId: null,
	contentUri: null,
	contentId: null,
	quorum: null,
	fastThreshold: null,
	slowThreshold: null,
	duration: null,
	partialPercentageSupportThreshold: null,
	universalPercentageSupportThreshold: null,
	flatSupportThreshold: null,
	disableFastPathAccessForNewMembers: null,
	executionGracePeriod: null,
	...over,
})

function proposal(over: Partial<ProposalWithVotes> = {}): ProposalWithVotes {
	return {
		id: P,
		spaceId: "22222222222222222222222222222222",
		name: null,
		proposedBy: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
		proposalVersion: 1,
		votingMode: "Fast",
		startTime: NOW - 3600n,
		endTime: NOW + 3600n,
		quorum: 1n,
		threshold: 2n,
		flatSupportThreshold: 2n,
		partialPercentageSupportThreshold: 0n,
		universalPercentageSupportThreshold: 0n,
		executeBy: null,
		totalEditors: 3n,
		executedAt: null,
		unexecutableAt: null,
		yesCount: 1n,
		noCount: 1n,
		abstainCount: 0n,
		votes: [],
		actions: [servedAction({actionType: "AddMember", targetId: TARGET})],
		...over,
	} as ProposalWithVotes
}

type Cards = Record<string, GeoCardRow[]>
/** Backlinks by (space, entity); cards by entity id. */
function client(cards: Cards, links: Record<string, GeoBacklinkRow[]>): GeoDbCardClient {
	return {
		readCard: async ({entityId}) => cards[entityId] ?? [],
		readBacklinks: async ({geoSpaceIds, entityId}) => links[`${geoSpaceIds[0]}:${entityId}`] ?? [],
	}
}

/** A proposal with one version, one action, and two vote records, as the feeder would have written them. */
function world(p = proposal()) {
	const ref = entityRef(p.id)
	const cards: Cards = {
		ver: versionRows(10),
		act: actionRows(ref, 10, 0, "AddMember", TARGET),
		oldAct: actionRows(ref, 5, 0, "Publish"),
		v1: [row(PROPOSAL_VOTE_COLUMNS.vote, "1"), row(PROPOSAL_VOTE_KEY_COLUMNS.version, "1")],
		v2: [row(PROPOSAL_VOTE_COLUMNS.vote, "2"), row(PROPOSAL_VOTE_KEY_COLUMNS.version, "1")],
		vOld: [row(PROPOSAL_VOTE_COLUMNS.vote, "1"), row(PROPOSAL_VOTE_KEY_COLUMNS.version, "0")],
	}
	const links: Record<string, GeoBacklinkRow[]> = {
		[`${GOV}:${ref}`]: [
			{prop: V.proposal, raw: ref, from: "ver"},
			{prop: A.proposal, raw: ref, from: "act"},
			{prop: A.proposal, raw: ref, from: "oldAct"},
		],
		[`${VOTES}:${ref}`]: ["v1", "v2", "vOld"].map((from) => ({
			prop: PROPOSAL_VOTE_KEY_COLUMNS.proposal,
			raw: ref,
			from,
		})),
	}
	return {cards, links}
}

describe("governance queries shadow", () => {
	const log = {warn: vi.fn()}
	const spaces = async (family: "governance" | "votes") => (family === "governance" ? GOV : VOTES)
	const cfg = readQueriesShadowConfig({GEODB_GOVERNANCE_QUERIES_SHADOW: "1"})
	const make = (
		c: GeoDbCardClient,
		over: Partial<typeof cfg> = {},
		systemSpaceOf: (family: "governance" | "votes") => Promise<string | null> = spaces,
	) => createGovernanceQueriesShadow({config: {...cfg, ...over}, geoDb: c, systemSpaceOf, log})
	const settle = () => new Promise((r) => setTimeout(r, 20))
	const metric = (line: string) =>
		Number(new RegExp(`^${line.replace(/[{}]/g, "\\$&")} (\\d+)$`, "m").exec(renderQueriesMetrics())?.[1])

	beforeEach(() => {
		__resetQueriesMetricsForTests()
		log.warn.mockClear()
	})

	it("is off unless the flag is on, and exports nothing until mounted", () => {
		expect(readQueriesShadowConfig({}).enabled).toBe(false)
		expect(readQueriesShadowConfig({GEODB_GOVERNANCE_QUERIES_SHADOW: "0"}).enabled).toBe(false)
		expect(readQueriesShadowConfig({GEODB_GOVERNANCE_QUERIES_SHADOW: "on"}).enabled).toBe(true)
		expect(renderQueriesMetrics()).toBe("")
	})

	it("does nothing and reads nothing when disabled", async () => {
		const reads = vi.fn()
		const c: GeoDbCardClient = {readCard: async () => (reads(), [])}
		make(c, {enabled: false}).detail(proposal())
		await settle()
		expect(reads).not.toHaveBeenCalled()
	})

	it("detail: matches on the current version's actions and votes, ignoring older versions", async () => {
		const p = proposal()
		const w = world(p)
		make(client(w.cards, w.links)).detail(p)
		await settle()
		expect(metric('gaia_api_governance_queries_shadow_total{query="detail",result="match"}')).toBe(1)
		expect(metric('gaia_api_governance_queries_shadow_total{query="list",result="match"}')).toBe(0)
	})

	it("detail: a vote count that differs is a mismatch, logged without ids", async () => {
		const p = proposal({yesCount: 2n})
		const w = world(p)
		make(client(w.cards, w.links)).detail(p)
		await settle()
		expect(metric('gaia_api_governance_queries_shadow_total{query="detail",result="mismatch"}')).toBe(1)
		const logged = JSON.stringify(log.warn.mock.calls)
		expect(logged).not.toContain(P)
		expect(logged).not.toContain(TARGET)
	})

	it("detail: an action whose type differs is a mismatch", async () => {
		const p = proposal({actions: [servedAction({actionType: "AddEditor", targetId: TARGET})]})
		const w = world(p)
		make(client(w.cards, w.links)).detail(p)
		await settle()
		expect(metric('gaia_api_governance_queries_shadow_total{query="detail",result="mismatch"}')).toBe(1)
	})

	it("detail: too many voters is unsupported, not a mismatch", async () => {
		const p = proposal()
		const w = world(p)
		make(client(w.cards, w.links), {voterMax: 2}).detail(p)
		await settle()
		expect(metric('gaia_api_governance_queries_shadow_total{query="detail",result="unsupported"}')).toBe(1)
		expect(
			metric('gaia_api_governance_queries_shadow_unsupported_total{query="detail",reason="too_many_records"}'),
		).toBe(1)
	})

	it("detail: a missing system space is unsupported", async () => {
		const p = proposal()
		const w = world(p)
		make(client(w.cards, w.links), {}, async () => null).detail(p)
		await settle()
		expect(
			metric(
				'gaia_api_governance_queries_shadow_unsupported_total{query="detail",reason="system_space_missing"}',
			),
		).toBe(1)
	})

	it("detail: a failing geo-db read is failed", async () => {
		const c: GeoDbCardClient = {
			readCard: async () => {
				throw new Error("boom")
			},
			readBacklinks: async () => [{prop: V.proposal, raw: "", from: "x"}],
		}
		make(c).detail(proposal())
		await settle()
		expect(metric('gaia_api_governance_queries_shadow_total{query="detail",result="failed"}')).toBe(1)
	})

	it("list: compares each listed proposal and the action-type filter, from the front up to listMax", async () => {
		const p = proposal() as unknown as ProposalListItem
		const w = world(proposal())
		const s = make(client(w.cards, w.links), {listMax: 2})
		s.list([p, p, p], {actionTypes: ["AddMember"]})
		await settle()
		expect(metric('gaia_api_governance_queries_shadow_total{query="list",result="match"}')).toBe(2)
		expect(
			metric(
				'gaia_api_governance_queries_shadow_uncompared_total{query="list",input="list_membership_from_postgres"}',
			),
		).toBe(2)
	})

	it("list: geo-db actions that break the filter are a mismatch", async () => {
		const p = proposal() as unknown as ProposalListItem
		const w = world(proposal())
		make(client(w.cards, w.links)).list([p], {excludeActionTypes: ["AddMember"]})
		await settle()
		expect(metric('gaia_api_governance_queries_shadow_total{query="list",result="mismatch"}')).toBe(1)
	})

	it("drops a comparison past maxInFlight as busy", async () => {
		const p = proposal()
		const w = world(p)
		const slow: GeoDbCardClient = {
			...client(w.cards, w.links),
			readBacklinks: () => new Promise(() => {}),
		}
		const s = make(slow, {maxInFlight: 1})
		s.detail(p)
		s.detail(p)
		await settle()
		expect(metric('gaia_api_governance_queries_shadow_unsupported_total{query="detail",reason="busy"}')).toBe(1)
	})

	describe("active checks", () => {
		const tRef = entityRef(TARGET)
		const activeWorld = (executed: boolean) => {
			const ref = entityRef(P4)
			const w = world(proposal({id: P4}))
			w.links[`${GOV}:${tRef}`] = [{prop: A.target, raw: tRef, from: "act"}]
			w.cards[proposalStatusRecordId(P4)] = executed
				? [row(PROPOSAL_STATUS_COLUMNS.executedAt, "instant:1700000000.000000")]
				: []
			expect(ref).toBe(P4)
			return w
		}

		it("member: served true and a not-executed AddMember proposal in geo-db is a match", async () => {
			const w = activeWorld(false)
			make(client(w.cards, w.links)).active("member", TARGET, true)
			await settle()
			expect(metric('gaia_api_governance_queries_shadow_total{query="active_member",result="match"}')).toBe(1)
			expect(
				metric(
					'gaia_api_governance_queries_shadow_uncompared_total{query="active_member",input="active_space_scope_from_postgres"}',
				),
			).toBe(1)
		})

		it("member: served true but only an executed proposal in geo-db is a mismatch", async () => {
			const w = activeWorld(true)
			make(client(w.cards, w.links)).active("member", TARGET, true)
			await settle()
			expect(metric('gaia_api_governance_queries_shadow_total{query="active_member",result="mismatch"}')).toBe(1)
		})

		it("member: served false with a not-executed candidate is unsupported, with its reason", async () => {
			const w = activeWorld(false)
			make(client(w.cards, w.links)).active("member", TARGET, false)
			await settle()
			expect(metric('gaia_api_governance_queries_shadow_total{query="active_member",result="unsupported"}')).toBe(
				1,
			)
			expect(
				metric(
					'gaia_api_governance_queries_shadow_unsupported_total{query="active_member",reason="status_inputs_not_read"}',
				),
			).toBe(1)
		})

		it("member: served false with no candidate is a match", async () => {
			make(client({}, {})).active("member", TARGET, false)
			await settle()
			expect(metric('gaia_api_governance_queries_shadow_total{query="active_member",result="match"}')).toBe(1)
		})

		it("editor: only AddEditor actions count, under its own query label", async () => {
			const w = activeWorld(false)
			make(client(w.cards, w.links)).active("editor", TARGET, true)
			await settle()
			// The action in the world is AddMember, so an editor check finds no candidate.
			expect(metric('gaia_api_governance_queries_shadow_total{query="active_editor",result="mismatch"}')).toBe(1)
		})
	})
})
