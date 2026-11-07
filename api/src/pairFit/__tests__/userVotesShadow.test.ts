import {beforeEach, describe, expect, it, vi} from "vitest"
import type {Database} from "../../forYou/queries"
import type {GeoBacklinkRow, GeoCardRow} from "../../geodb/card"
import {GeoDbError} from "../../geodb/client"
import {USER_VOTE_COLUMNS} from "../../geodb/keyedRecordsShadow"
import {entityRef} from "../../proposals/governance/recordKey"
import {
	__resetUserVotesTwinMetricsForTests,
	createUserVotesShadow,
	readUserVotesShadowConfig,
	renderUserVotesTwinMetrics,
	type UserVotesShadowConfig,
} from "../userVotesShadow"
import {
	decodeVoteRecord,
	heldStances,
	readEntityVotedByGeoDb,
	readSharedClaimsGeoDb,
	readUserVotes,
	sameSharedClaims,
	sameVotedBy,
	sharedClaimsOf,
	type VotesReader,
	votedByRefs,
} from "../userVotesTwin"

const SPACE = "a".repeat(32)
const U = "11111111111141118111111111111111"
const C1 = "22222222222242228222222222222222"
const C2 = "33333333333343338333333333333333"
const CLAIM1 = "44444444444444444444444444444444" // non-v4: remapped by entityRef
const CLAIM2 = "55555555555545558555555555555555"
const CLAIM3 = "66666666666646668666666666666666"

type V = {user: string; object: string | null; objectType?: number; kind?: number; direction: number; at: number}

/** A fake votes Space: records indexed by user, served through the two reads the twin makes. */
function fakeGeoDb(votes: V[], opts: {backlinkLimitHit?: boolean} = {}): VotesReader & {backlinkCalls: number} {
	const records = votes.map((v, i) => ({id: i.toString(16).padStart(32, "0"), v}))
	const fake = {
		backlinkCalls: 0,
		async readBacklinks({entityId, limit}: {geoSpaceIds: string[]; entityId: string; limit: number}) {
			fake.backlinkCalls++
			const rows: GeoBacklinkRow[] = records
				.filter((r) => entityRef(r.v.user) === entityId)
				.map((r) => ({prop: USER_VOTE_COLUMNS.user, raw: entityId, from: r.id}))
			// Rows that refer to the user through another column are not vote records.
			rows.push({prop: "f".repeat(32), raw: entityId, from: "e".repeat(32)})
			return opts.backlinkLimitHit ? rows.slice(0, 1).concat(Array(limit).fill(rows[0])) : rows
		},
		async readCards({entityIds}: {geoSpaceIds: string[]; entityIds: string[]}) {
			const rows: GeoCardRow[] = []
			for (const r of records) {
				if (!entityIds.includes(r.id)) continue
				const C = USER_VOTE_COLUMNS
				const add = (prop: string, raw: string) => rows.push({prop, raw, entity: r.id})
				add(C.user, entityRef(r.v.user))
				if (r.v.object !== null) add(C.object, entityRef(r.v.object))
				add(C.space, "b".repeat(32))
				add(C.objectType, String(r.v.objectType ?? 0))
				add(C.kind, `${r.v.kind ?? 1}.0`)
				add(C.direction, String(r.v.direction))
				add(C.votedAt, `instant:${r.v.at}.000000000`)
			}
			return rows
		},
	}
	return fake
}

const votes: V[] = [
	{user: U, object: CLAIM1, direction: 0, at: 100},
	{user: U, object: CLAIM2, direction: 1, at: 100},
	{user: U, object: CLAIM3, direction: 0, at: 100, kind: 0}, // curation: not a stance
	{user: C1, object: CLAIM1, direction: 1, at: 100}, // opposite
	{user: C1, object: CLAIM2, direction: 1, at: 100}, // same side
	{user: C2, object: CLAIM3, direction: 0, at: 100, kind: 0}, // shares nothing
]

describe("decodeVoteRecord", () => {
	it("returns null for a card that lacks a column and throws on two values", () => {
		expect(decodeVoteRecord([])).toBeNull()
		const C = USER_VOTE_COLUMNS
		const two: GeoCardRow[] = [
			{prop: C.user, raw: "a"},
			{prop: C.user, raw: "b"},
		]
		expect(() => decodeVoteRecord(two)).toThrow()
	})
})

describe("the SQL's rule", () => {
	const vote = (over: Partial<Parameters<typeof heldStances>[0][number]>) => ({
		user: "u",
		object: "o",
		objectType: 0,
		space: "s",
		kind: 1,
		direction: 0,
		votedAt: 1n,
		...over,
	})
	it("keeps the latest stance across spaces and ignores kinds, relation votes and removals", () => {
		const held = heldStances([
			vote({votedAt: 1n, direction: 0}),
			vote({votedAt: 5n, direction: 1, space: "t"}),
			vote({object: "p", kind: 2}),
			vote({object: "q", objectType: 1}),
			vote({object: "r", direction: 2}),
		])
		expect([...held]).toEqual([["o", 1]])
	})
	it("filters before picking the latest, so a later removal does not hide an earlier stance", () => {
		expect([...heldStances([vote({votedAt: 1n, direction: 0}), vote({votedAt: 9n, direction: 2})])]).toEqual([
			["o", 0],
		])
	})
	it("reports shared claims and sides", () => {
		const a = new Map<string, 0 | 1>([
			["x", 0],
			["y", 1],
		])
		const b = new Map<string, 0 | 1>([
			["y", 1],
			["x", 1],
			["z", 0],
		])
		expect(sharedClaimsOf(a, b)).toEqual([
			{claimRef: "x", opposite: true},
			{claimRef: "y", opposite: false},
		])
	})
	it("narrows votedBy by kind and direction, any when empty, claims only", () => {
		const vs = [
			vote({object: "a", kind: 1, direction: 0}),
			vote({object: "b", kind: 2, direction: 2}),
			vote({object: "c", kind: 0, direction: 0}),
			vote({object: "d", objectType: 1}),
		]
		expect(votedByRefs(vs, {})).toEqual(["a", "b", "c"])
		expect(votedByRefs(vs, {kinds: [1, 2], types: [0, 1]})).toEqual(["a"])
		expect(votedByRefs(vs, {kinds: [2]})).toEqual(["b"])
	})
})

describe("readers", () => {
	it("reads one user's votes through backlinks and a card read, dropping non-vote backlinks", async () => {
		const g = fakeGeoDb(votes)
		const r = await readUserVotes(g, SPACE, U)
		expect(r.votes).toHaveLength(3)
		expect(r.truncated).toBe(false)
	})
	it("answers shared claims as the SQL does, with an entry only where something is shared", async () => {
		const {byCandidate, truncated} = await readSharedClaimsGeoDb(fakeGeoDb(votes), SPACE, U, [C1, C2])
		expect(truncated).toBe(false)
		expect([...byCandidate.keys()]).toEqual([C1])
		expect(byCandidate.get(C1)).toEqual(
			[
				{claimRef: entityRef(CLAIM1), opposite: true},
				{claimRef: entityRef(CLAIM2), opposite: false},
			].sort((a, b) => (a.claimRef < b.claimRef ? -1 : 1)),
		)
	})
	it("flags a read that filled the backlink limit", async () => {
		const r = await readSharedClaimsGeoDb(fakeGeoDb(votes, {backlinkLimitHit: true}), SPACE, U, [C1], 2)
		expect(r.truncated).toBe(true)
	})
	it("serves votedBy for positions", async () => {
		const {refs} = await readEntityVotedByGeoDb(fakeGeoDb(votes), SPACE, U, {kinds: [1, 2], types: [0, 1]})
		expect(refs).toEqual([entityRef(CLAIM1), entityRef(CLAIM2)].sort())
	})
	it("rejects when the client cannot read backlinks", async () => {
		await expect(readUserVotes({} as VotesReader, SPACE, U)).rejects.toBeInstanceOf(GeoDbError)
	})
})

describe("comparisons", () => {
	it("maps Postgres ids to refs and compares sides", () => {
		const twin = [{claimRef: entityRef(CLAIM1), opposite: true}]
		expect(sameSharedClaims([{claimId: CLAIM1, opposite: true}], twin)).toBe(true)
		expect(sameSharedClaims([{claimId: CLAIM1, opposite: false}], twin)).toBe(false)
		expect(sameSharedClaims([], twin)).toBe(false)
		expect(sameSharedClaims([], [])).toBe(true)
	})
	it("compares votedBy as sets of refs", () => {
		const refs = [entityRef(CLAIM1), entityRef(CLAIM2)].sort()
		expect(sameVotedBy([CLAIM2, CLAIM1, CLAIM1], refs)).toBe(true)
		expect(sameVotedBy([CLAIM1], refs)).toBe(false)
	})
})

describe("config", () => {
	it("is off unless the flag says on", () => {
		expect(readUserVotesShadowConfig({}).enabled).toBe(false)
		expect(readUserVotesShadowConfig({GEODB_USER_VOTES_TWIN: "0"}).enabled).toBe(false)
		expect(readUserVotesShadowConfig({GEODB_USER_VOTES_TWIN: "true"}).enabled).toBe(true)
		expect(readUserVotesShadowConfig({GEODB_USER_VOTES_TWIN_SAMPLE: "999"})).toMatchObject({
			sample: 25,
			intervalMs: 10_000,
		})
	})
})

describe("the shadow", () => {
	const config: UserVotesShadowConfig = {enabled: true, maxInFlight: 1, intervalMs: 1000, sample: 5}
	const log = {warn: vi.fn()}
	const settle = () => new Promise((r) => setTimeout(r, 10))
	const count = (read: string, result: string) =>
		Number(
			new RegExp(`gaia_api_user_votes_twin_total\\{read="${read}",result="${result}"\\} (\\d+)`).exec(
				renderUserVotesTwinMetrics(),
			)?.[1],
		)
	/** The Postgres votedBy read: the positions the fake holds for U. */
	const dbOf = (ids: string[]): Database =>
		({execute: async () => ({rows: ids.map((object_id) => ({object_id}))})}) as unknown as Database
	const served = new Map([
		[
			C1,
			[
				{claimId: CLAIM1, opposite: true, topicIds: []},
				{claimId: CLAIM2, opposite: false, topicIds: []},
			],
		],
	])
	const shadow = (over: Partial<Parameters<typeof createUserVotesShadow>[0]> = {}) =>
		createUserVotesShadow({
			config,
			db: dbOf([CLAIM1, CLAIM2]),
			geoDb: fakeGeoDb(votes),
			systemSpaceOf: async () => SPACE,
			log,
			...over,
		})

	beforeEach(() => {
		__resetUserVotesTwinMetricsForTests()
		log.warn.mockClear()
	})

	it("exports nothing until mounted", () => {
		expect(renderUserVotesTwinMetrics()).toBe("")
	})

	it("counts a match per candidate and for votedBy", async () => {
		shadow().compare(U, [C1, C2], served)
		await settle()
		expect(count("shared_claims", "match")).toBe(2)
		expect(count("voted_by", "match")).toBe(1)
		expect(count("shared_claims", "mismatch")).toBe(0)
	})

	it("counts a mismatch when geo-db lacks what Postgres served, and logs counts only", async () => {
		shadow({geoDb: fakeGeoDb(votes.filter((v) => v.user !== C1))}).compare(U, [C1], served)
		await settle()
		expect(count("shared_claims", "mismatch")).toBe(1)
		expect(JSON.stringify(log.warn.mock.calls)).not.toContain(C1)
	})

	it("counts unsupported when the votes Space is not founded", async () => {
		shadow({systemSpaceOf: async () => null}).compare(U, [C1], served)
		await settle()
		expect(count("shared_claims", "unsupported")).toBe(1)
		expect(renderUserVotesTwinMetrics()).toContain('reason="no_votes_space"} 2')
	})

	it("counts failed when a read throws, never throwing to the caller", async () => {
		const geoDb = fakeGeoDb(votes)
		geoDb.readBacklinks = async () => {
			throw new GeoDbError("timeout", "slow")
		}
		shadow({geoDb}).compare(U, [C1], served)
		await settle()
		expect(count("shared_claims", "failed")).toBe(1)
		expect(count("voted_by", "failed")).toBe(1)
	})

	it("is throttled and bounded: one pass per interval, busy while one runs", async () => {
		let t = 0
		const s = shadow({now: () => t, geoDb: fakeGeoDb(votes)})
		s.compare(U, [C1], served)
		s.compare(U, [C1], served) // within the interval: dropped
		t = 5000
		s.compare(U, [C1], served) // first still in flight: busy
		await settle()
		expect(count("shared_claims", "match")).toBe(1)
		expect(renderUserVotesTwinMetrics()).toContain('reason="busy"} 2')
	})

	it("does nothing when the flag is off", async () => {
		const g = fakeGeoDb(votes)
		shadow({config: {...config, enabled: false}, geoDb: g}).compare(U, [C1], served)
		await settle()
		expect(g.backlinkCalls).toBe(0)
	})
})
