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
const settle = () => new Promise((r) => setTimeout(r, 0))

function make(
	rows: () => Promise<GeoCardRow[]>,
	over: {
		space?: string | null
		enabled?: boolean
		intervalMs?: number
		maxInFlight?: number
		now?: () => number
	} = {},
) {
	const warn = vi.fn()
	const geoDb: GeoDbCardClient = {readCard: vi.fn(rows)}
	const shadow = createFeedExperimentShadow({
		config: {enabled: over.enabled ?? true, maxInFlight: over.maxInFlight ?? 2, intervalMs: over.intervalMs ?? 0},
		geoDb,
		systemSpaceOf: async () => (over.space === undefined ? "a".repeat(32) : over.space),
		log: {warn},
		now: over.now,
	})
	return {shadow, geoDb, warn}
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
	it("is silent and empty until mounted, and off does nothing", async () => {
		expect(renderFeedExperimentMetrics()).toBe("")
		const {shadow, geoDb} = make(async () => card(), {enabled: false})
		shadow.compare(served(), USER)
		await settle()
		expect(geoDb.readCard).not.toHaveBeenCalled()
		expect(metric('result="match"')).toBe("0")
	})
	it("counts a match and a mismatch, reading the served experiment's record", async () => {
		const a = make(async () => card())
		a.shadow.compare(served(), USER)
		await settle()
		expect(metric('result="match"')).toBe("1")
		expect(a.geoDb.readCard).toHaveBeenCalledWith({
			geoSpaceId: "a".repeat(32),
			entityId: feedExperimentRecordId("exp-1"),
		})
		const b = make(async () => card({armB: "best"}))
		b.shadow.compare(served(), USER)
		await settle()
		expect(metric('result="mismatch"')).toBe("1")
		expect(b.warn).toHaveBeenCalled()
	})
	it("counts unsupported for a missing Space, a missing record and an ambiguous one", async () => {
		make(async () => card(), {space: null}).shadow.compare(served(), USER)
		make(async () => []).shadow.compare(served(), USER)
		make(async () => card().filter((r) => r.prop !== COL.salt)).shadow.compare(served(), USER)
		await settle()
		expect(metric('result="unsupported"')).toBe("3")
		for (const r of ["system_space_missing", "record_missing", "ambiguous_record"])
			expect(metric(`reason="${r}"`)).toBe("1")
	})
	it("compares a member's value with the member record, not the hash", async () => {
		const reads = (member: GeoCardRow[]) => {
			const geoDb: GeoDbCardClient = {
				readCard: vi.fn(async ({entityId}) =>
					entityId === feedExperimentMemberRecordId("exp-1", USER) ? member : card(),
				),
			}
			const shadow = createFeedExperimentShadow({
				config: {enabled: true, maxInFlight: 2, intervalMs: 0},
				geoDb,
				systemSpaceOf: async () => "a".repeat(32),
				log: {warn: vi.fn()},
			})
			return {shadow, geoDb}
		}
		// The served value disagrees with the hash on purpose: a member's value wins.
		const memberServed = served({assignment: "member", interleaved: !interleaved})
		const a = reads([{prop: MCOL.interleaved, raw: String(!interleaved)}])
		a.shadow.compare(memberServed, USER)
		await settle()
		expect(metric('result="match"')).toBe("1")
		expect(a.geoDb.readCard).toHaveBeenCalledTimes(2)
		reads([{prop: MCOL.interleaved, raw: String(interleaved)}]).shadow.compare(memberServed, USER)
		await settle()
		expect(metric('result="mismatch"')).toBe("1")
		reads([]).shadow.compare(memberServed, USER)
		await settle()
		expect(metric('reason="member_record_missing"')).toBe("1")
		// A hash-assigned user reads only the experiment record.
		const h = reads([])
		h.shadow.compare(served(), USER)
		await settle()
		expect(h.geoDb.readCard).toHaveBeenCalledTimes(1)
	})
	it("counts a failed read without throwing", async () => {
		make(async () => {
			throw new GeoDbError("timeout", "down")
		}).shadow.compare(served(), USER)
		await settle()
		expect(metric('result="failed"')).toBe("1")
	})
	it("compares at most once an interval, and drops past maxInFlight", async () => {
		let t = 0
		const a = make(async () => card(), {intervalMs: 1000, now: () => t})
		a.shadow.compare(served(), USER)
		a.shadow.compare(served(), USER)
		await settle()
		expect(a.geoDb.readCard).toHaveBeenCalledTimes(1)
		t = 1000
		a.shadow.compare(served(), USER)
		await settle()
		expect(a.geoDb.readCard).toHaveBeenCalledTimes(2)
		const b = make(() => new Promise<GeoCardRow[]>(() => {}), {maxInFlight: 1})
		b.shadow.compare(served(), USER)
		await settle()
		b.shadow.compare(served(), USER)
		expect(metric('reason="busy"')).toBe("1")
	})
	it("reads its flag off by default", () => {
		expect(readFeedExperimentShadowConfig({}).enabled).toBe(false)
		expect(readFeedExperimentShadowConfig({GEODB_FEED_EXPERIMENT_SHADOW: "1"}).enabled).toBe(true)
		expect(readFeedExperimentShadowConfig({GEODB_ENTITY_RANKING_CONFIG_SHADOW: "1"}).enabled).toBe(false)
	})
})
