import {createHash} from "node:crypto"
import {existsSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"

import {beforeEach, describe, expect, it, vi} from "vitest"

import type {GeoCardRow} from "../../geodb/card"
import {type GeoDbCardClient, GeoDbError} from "../../geodb/client"
import {
	__resetFeedExperimentMetricsForTests,
	FEED_EXPERIMENT_COLUMNS as COL,
	createFeedExperimentShadow,
	decodeExperiment,
	decodeMemberInterleaved,
	experimentDiffs,
	FEED_EXPERIMENT_KIND,
	FEED_EXPERIMENT_MEMBER_KIND,
	feedExperimentMemberRecordId,
	feedExperimentRecordId,
	hashInterleaved,
	FEED_EXPERIMENT_MEMBER_COLUMNS as MCOL,
	readFeedExperimentShadowConfig,
	renderFeedExperimentMetrics,
} from "../feedExperimentShadow"
import type {FeedExperiment} from "../queries"

const here = dirname(fileURLToPath(import.meta.url))
const USER = "550e8400-e29b-41d4-a716-446655440000"
const SALT = "0123456789abcdef"
const SHARE = "0.5"
const card = (over: Partial<Record<keyof typeof COL, string>> = {}): GeoCardRow[] => {
	const v = {id: "exp-1", active: "true", armA: "best", armB: "for-you", share: SHARE, salt: SALT, ...over}
	return (Object.keys(COL) as (keyof typeof COL)[]).map((k) => ({prop: COL[k], raw: v[k]}))
}
const interleaved = hashInterleaved(SALT, USER, Number(SHARE))
const served = (over: Partial<FeedExperiment> = {}): FeedExperiment => ({
	id: "exp-1",
	arms: ["best", "for-you"],
	interleaved,
	assignment: "hash",
	...over,
})
const metric = (name: string) => new RegExp(`${name}\\} (\\d+)`).exec(renderFeedExperimentMetrics())?.[1]

const timeout = Object.assign(new Error("canceling statement due to statement timeout"), {code: "57014"})
const MEMBER = "11111111-2222-4333-8444-555555555555"

function make(
	rows: (entityId: string) => Promise<GeoCardRow[]>,
	over: {
		space?: string | null
		enabled?: boolean
		experiment?: (userId: string) => Promise<FeedExperiment | null>
		member?: string | null
	} = {},
) {
	const warn = vi.fn()
	const geoDb: GeoDbCardClient = {readCard: vi.fn(({entityId}) => rows(entityId))}
	const readExperiment = vi.fn((_db: unknown, userId: string) => (over.experiment ?? (async () => served()))(userId))
	const shadow = createFeedExperimentShadow({
		config: {enabled: over.enabled ?? true, intervalMs: 1000},
		db: {execute: vi.fn()},
		geoDb,
		systemSpaceOf: async () => (over.space === undefined ? "a".repeat(32) : over.space),
		log: {warn},
		readExperiment,
		sampleMember: async () => (over.member === undefined ? null : over.member),
		randomUserId: () => USER,
	})
	return {shadow, geoDb, warn, readExperiment}
}

beforeEach(() => __resetFeedExperimentMetricsForTests())

describe("pins", () => {
	it("match the feeder's recordKinds (kind and every column)", () => {
		const file = join(here, "../../../../geodb-feeder/src/recordKinds.ts")
		if (!existsSync(file)) return
		const text = readFileSync(file, "utf8")
		expect(text).toContain(`feedExperiment: {id: "${FEED_EXPERIMENT_KIND}"`)
		const block = text.slice(text.indexOf("\tfeedExperiment: {\n"))
		for (const [name, id] of Object.entries(COL)) expect(block).toContain(`${name}: "${id}"`)
	})
	it("match the feeder's member kind and columns", () => {
		const file = join(here, "../../../../geodb-feeder/src/recordKinds.ts")
		if (!existsSync(file)) return
		const text = readFileSync(file, "utf8")
		expect(text).toContain(`feedExperimentMember: {id: "${FEED_EXPERIMENT_MEMBER_KIND}"`)
		const block = text.slice(text.indexOf("\tfeedExperimentMember: {\n"))
		for (const [name, id] of Object.entries(MCOL)) expect(block).toContain(`${name}: "${id}"`)
	})
	it("keys the member record by experiment and user, dashed or not", () => {
		expect(feedExperimentMemberRecordId("exp-1", USER)).toBe(
			feedExperimentMemberRecordId("exp-1", USER.replaceAll("-", "")),
		)
		expect(feedExperimentMemberRecordId("exp-1", USER)).not.toBe(feedExperimentMemberRecordId("exp-2", USER))
		expect(feedExperimentMemberRecordId("exp-1", USER)).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
	})
	it("keys the record by the experiment id, as a version-4 uuid", () => {
		expect(feedExperimentRecordId("exp-1")).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
		expect(feedExperimentRecordId("exp-1")).not.toBe(feedExperimentRecordId("exp-2"))
	})
})

describe("hashInterleaved", () => {
	it("is the unsigned first 32 bits of md5(salt:user) over 2^32, below share", () => {
		const bucket = Number.parseInt(createHash("md5").update(`${SALT}:${USER}`).digest("hex").slice(0, 8), 16)
		expect(hashInterleaved(SALT, USER, (bucket + 1) / 4294967296)).toBe(true)
		expect(hashInterleaved(SALT, USER, bucket / 4294967296)).toBe(false)
		expect(hashInterleaved(SALT, USER, 0)).toBe(false)
		expect(hashInterleaved(SALT, USER, 1)).toBe(true)
	})
})

describe("decodeExperiment", () => {
	it("reads the columns", () => {
		expect(decodeExperiment(card())).toEqual({active: true, armA: "best", armB: "for-you", share: 0.5, salt: SALT})
	})
	it("is null for no record, and refuses a missing, double or malformed column", () => {
		expect(decodeExperiment([])).toBeNull()
		expect(() => decodeExperiment(card().filter((r) => r.prop !== COL.salt))).toThrow()
		expect(() => decodeExperiment([...card(), {prop: COL.armA, raw: "for-you"}])).toThrow()
		expect(() => decodeExperiment(card({active: "yes"}))).toThrow()
		expect(() => decodeExperiment(card({share: "x"}))).toThrow()
	})
})

describe("decodeMemberInterleaved", () => {
	it("reads the flag, null for no record, and refuses a missing, double or malformed one", () => {
		expect(decodeMemberInterleaved([{prop: MCOL.interleaved, raw: "true"}])).toBe(true)
		expect(decodeMemberInterleaved([{prop: MCOL.interleaved, raw: "false"}])).toBe(false)
		expect(decodeMemberInterleaved([])).toBeNull()
		expect(() => decodeMemberInterleaved([{prop: MCOL.user, raw: "x"}])).toThrow()
		expect(() =>
			decodeMemberInterleaved([
				{prop: MCOL.interleaved, raw: "true"},
				{prop: MCOL.interleaved, raw: "false"},
			]),
		).toThrow()
		expect(() => decodeMemberInterleaved([{prop: MCOL.interleaved, raw: "1"}])).toThrow()
	})
})

describe("experimentDiffs", () => {
	const held = decodeExperiment(card())!
	it("is empty when everything agrees", () => expect(experimentDiffs(served(), held, USER)).toEqual([]))
	it("names an inactive record, different arms and a different hash assignment", () => {
		expect(experimentDiffs(served(), {...held, active: false}, USER)).toEqual(["active"])
		expect(experimentDiffs(served({arms: ["for-you", "best"]}), held, USER)).toEqual(["arms"])
		expect(experimentDiffs(served({interleaved: !interleaved}), held, USER)).toEqual(["interleaved"])
	})
	it("does not compare a member's value (members are not mirrored)", () => {
		expect(experimentDiffs(served({assignment: "member", interleaved: !interleaved}), held, USER)).toEqual([])
	})
})

describe("shadow", () => {
	it("is silent and empty until mounted, and off starts no timer", async () => {
		expect(renderFeedExperimentMetrics()).toBe("")
		const spy = vi.spyOn(globalThis, "setInterval")
		const {shadow, geoDb} = make(async () => card(), {enabled: false})
		shadow.start()()
		expect(spy).not.toHaveBeenCalled()
		expect(geoDb.readCard).not.toHaveBeenCalled()
		expect(metric('result="match"')).toBe("0")
		spy.mockRestore()
	})
	it("counts a match and a mismatch for a sampled hash user, reading the active experiment's record", async () => {
		const a = make(async () => card())
		await a.shadow.runOnce()
		expect(metric('result="match"')).toBe("1")
		expect(a.readExperiment).toHaveBeenCalledWith(expect.anything(), USER)
		expect(a.geoDb.readCard).toHaveBeenCalledWith({
			geoSpaceId: "a".repeat(32),
			entityId: feedExperimentRecordId("exp-1"),
		})
		const b = make(async () => card({armB: "best"}))
		await b.shadow.runOnce()
		expect(metric('result="mismatch"')).toBe("1")
		expect(b.warn).toHaveBeenCalled()
	})
	it("compares nothing when no experiment is active", async () => {
		const a = make(async () => card(), {experiment: async () => null})
		await a.shadow.runOnce()
		expect(a.geoDb.readCard).not.toHaveBeenCalled()
		expect(metric('result="match"')).toBe("0")
	})
	it("counts unsupported for a missing Space, a missing record and an ambiguous one", async () => {
		await make(async () => card(), {space: null}).shadow.runOnce()
		await make(async () => []).shadow.runOnce()
		await make(async () => card().filter((r) => r.prop !== COL.salt)).shadow.runOnce()
		expect(metric('result="unsupported"')).toBe("3")
		for (const r of ["system_space_missing", "record_missing", "ambiguous_record"])
			expect(metric(`reason="${r}"`)).toBe("1")
	})
	it("also samples a member and compares its value with the member record, not the hash", async () => {
		const reads = (member: GeoCardRow[]) =>
			make(async (entityId) => (entityId === feedExperimentMemberRecordId("exp-1", MEMBER) ? member : card()), {
				member: MEMBER,
				// The served value disagrees with the hash on purpose: a member's value wins.
				experiment: async (userId) =>
					userId === MEMBER ? served({assignment: "member", interleaved: !memberHash}) : served(),
			})
		const memberHash = hashInterleaved(SALT, MEMBER, Number(SHARE))
		const a = reads([{prop: MCOL.interleaved, raw: String(!memberHash)}])
		await a.shadow.runOnce()
		// the hash user and the member both match
		expect(metric('result="match"')).toBe("2")
		expect(a.geoDb.readCard).toHaveBeenCalledTimes(3)
		await reads([{prop: MCOL.interleaved, raw: String(memberHash)}]).shadow.runOnce()
		expect(metric('result="mismatch"')).toBe("1")
		await reads([]).shadow.runOnce()
		expect(metric('reason="member_record_missing"')).toBe("1")
	})
	it("counts a geo-db failure as failed, a Postgres one as reference_timeout or reference_failed", async () => {
		await make(async () => {
			throw new GeoDbError("timeout", "down")
		}).shadow.runOnce()
		expect(metric('result="failed"')).toBe("1")
		const slow = make(async () => card(), {
			experiment: async () => {
				throw timeout
			},
		})
		await slow.shadow.runOnce()
		expect(metric('result="reference_timeout"')).toBe("1")
		expect(slow.geoDb.readCard).not.toHaveBeenCalled()
		await make(async () => card(), {
			experiment: async () => {
				throw new Error("connection refused")
			},
		}).shadow.runOnce()
		expect(metric('result="reference_failed"')).toBe("1")
		expect(metric('result="failed"')).toBe("1")
	})
	it("runs on the timer without a request, never overlaps itself, and stops", async () => {
		vi.useFakeTimers()
		try {
			let release: (rows: GeoCardRow[]) => void = () => {}
			const a = make(
				() =>
					new Promise<GeoCardRow[]>((r) => {
						release = r
					}),
			)
			const stop = a.shadow.start()
			await vi.advanceTimersByTimeAsync(1000)
			expect(a.geoDb.readCard).toHaveBeenCalledTimes(1)
			await vi.advanceTimersByTimeAsync(2000)
			expect(a.geoDb.readCard).toHaveBeenCalledTimes(1)
			expect(metric('reason="busy"')).toBe("2")
			release(card())
			await vi.advanceTimersByTimeAsync(0)
			expect(metric('result="match"')).toBe("1")
			stop()
			await vi.advanceTimersByTimeAsync(5000)
			expect(a.geoDb.readCard).toHaveBeenCalledTimes(1)
		} finally {
			vi.useRealTimers()
		}
	})
	it("reads its flag off by default, and a five-minute interval", () => {
		expect(readFeedExperimentShadowConfig({}).enabled).toBe(false)
		expect(readFeedExperimentShadowConfig({GEODB_FEED_EXPERIMENT_SHADOW: "1"}).enabled).toBe(true)
		expect(readFeedExperimentShadowConfig({GEODB_ENTITY_RANKING_CONFIG_SHADOW: "1"}).enabled).toBe(false)
		expect(readFeedExperimentShadowConfig({}).intervalMs).toBe(300_000)
	})
})
