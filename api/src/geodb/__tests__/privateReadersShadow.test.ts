/**
 * The shadow's plumbing: config reads, the restricted / absent / truncated paths, the comparisons and the metrics. The
 * twin's arithmetic is pinned against Postgres in privateReaders.golden.test.ts. Synthetic ids only.
 */
import {existsSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {FOR_YOU_CONFIG_COLUMNS, forYouConfigRecordId} from "../../forYou/configShadow"
import type {Database} from "../../forYou/queries"
import type {GeoCardRow} from "../card"
import {GeoDbError} from "../client"
import {INTEREST_SIGNAL_KINDS, interestConfigRecordId, interestSignalWeightRecordId} from "../configRevisionShadow"
import type {DerivedScan, DerivedScanRow} from "../derived"
import {
	groupHex,
	INTEREST_CONFIG_COLUMNS,
	INTEREST_SIGNAL_WEIGHT_COLUMNS,
	KIND_BY_TAG,
	type TopicWeightRow,
} from "../privateReaders"
import {
	__resetPrivateReadersShadowMetricsForTests,
	createPrivateReadersShadow,
	MAX_SPREAD_SCANS,
	type ReferenceSide,
	readPrivateReadersShadowConfig,
	readTwinConfig,
	renderPrivateReadersShadowMetrics,
	sameCounts,
	sameOwnWeights,
	sameWeights,
	twinSeen,
	twinWeights,
} from "../privateReadersShadow"

const here = dirname(fileURLToPath(import.meta.url))
const USER = "00000000-0000-4000-8000-000000000001"
const TOPIC_A = "00000000-0000-4000-8000-0000000000a1"
const TOPIC_B = "00000000-0000-4000-8000-0000000000b2"
const OBJECT = "00000000-0000-4000-8000-0000000000c3"
const ITEM = "00000000-0000-4000-8000-0000000000d4"
const AS_OF = new Date("2026-11-06T12:00:00Z")
const asOfUs = BigInt(AS_OF.getTime()) * 1000n

const metric = (reader: string, result: string) =>
	Number(
		new RegExp(`reader="${reader}",result="${result}"\\} (\\d+)`).exec(renderPrivateReadersShadowMetrics())?.[1] ??
			-1,
	)
const reasonMetric = (reason: string) =>
	Number(new RegExp(`reason="${reason}"\\} (\\d+)`).exec(renderPrivateReadersShadowMetrics())?.[1] ?? -1)

const i32le = (n: number) => {
	const b = Buffer.alloc(4)
	b.writeInt32LE(n)
	return b.toString("hex")
}
const i64le = (n: bigint) => {
	const b = Buffer.alloc(8)
	b.writeBigInt64LE(n)
	return b.toString("hex")
}
const signalRow = (topic: string, kind: string, decayed: number): DerivedScanRow => ({
	score: decayed,
	keyHex:
		groupHex(USER) +
		groupHex(topic) +
		KIND_BY_TAG.indexOf(kind as never)
			.toString(16)
			.padStart(2, "0"),
	valueHex: i32le(3) + i64le(asOfUs) + groupHex(OBJECT) + i64le(asOfUs),
})
const coocRow = (topic: string, neighbour: string, similarity: number): DerivedScanRow => ({
	score: similarity,
	keyHex: groupHex(topic) + groupHex(neighbour),
	valueHex: i32le(2),
})
const seenRow = (at: Date, impressions: number): DerivedScanRow => {
	const day = Buffer.alloc(4)
	day.writeInt32BE(Math.floor(at.getTime() / 86400000))
	const us = BigInt(at.getTime()) * 1000n
	return {
		score: Number(us) / 1e6,
		keyHex: groupHex(USER) + day.toString("hex") + groupHex(ITEM),
		valueHex: i32le(impressions) + i64le(us),
	}
}

const row = (prop: string, raw: string): GeoCardRow => ({prop, raw}) as GeoCardRow
const configCards = (overrides: Record<string, GeoCardRow[]> = {}): Map<string, GeoCardRow[]> => {
	const cards = new Map<string, GeoCardRow[]>([
		[
			interestConfigRecordId(),
			[row(INTEREST_CONFIG_COLUMNS.halfLifeDays, "30"), row(INTEREST_CONFIG_COLUMNS.spreadFraction, "0.5")],
		],
		[
			forYouConfigRecordId(),
			[
				row(FOR_YOU_CONFIG_COLUMNS.maxBoostDays, "7"),
				row(FOR_YOU_CONFIG_COLUMNS.interestHalfSaturation, "1"),
				row(FOR_YOU_CONFIG_COLUMNS.explorationShare, "0.1"),
				row(FOR_YOU_CONFIG_COLUMNS.maxTopics, "50"),
			],
		],
		...INTEREST_SIGNAL_KINDS.map(
			(k) =>
				[
					interestSignalWeightRecordId(k),
					[
						row(INTEREST_SIGNAL_WEIGHT_COLUMNS.weight, "1"),
						row(INTEREST_SIGNAL_WEIGHT_COLUMNS.decays, "false"),
					],
				] as [string, GeoCardRow[]],
		),
	])
	for (const [k, v] of Object.entries(overrides)) cards.set(k, v)
	return cards
}
const geoDbOf = (cards = configCards()) => ({
	readCard: async ({entityId}: {entityId: string}) => cards.get(entityId) ?? [],
})
const systemSpaceOf = async () => "config-space"

/** A scan reader over fixed groups; unknown groups are empty. */
function readerOf(groups: Record<string, DerivedScan | Error>) {
	return {
		scan: vi.fn(async (collection: number, group: string) => {
			const got = groups[`${collection}:${group}`]
			if (got instanceof Error) throw got
			return got ?? {rows: [], more: false}
		}),
	}
}
const refused = () => new GeoDbError("client_failed", "derived read refused: code=NotFoundOrRestricted collection 28")

const pgRow = (over: Partial<TopicWeightRow> = {}): TopicWeightRow => ({
	topicId: TOPIC_A,
	weight: 1,
	ownWeight: 1,
	relatedWeight: 0,
	topKind: "vote",
	topKindCount: 3,
	topObjectId: OBJECT,
	relatedViaTopicId: null,
	...over,
})

beforeEach(() => __resetPrivateReadersShadowMetricsForTests())

describe("pins", () => {
	it("use the feeder's column ids", () => {
		const file = join(here, "../../../../geodb-feeder/src/recordKinds.ts")
		if (!existsSync(file)) return
		const src = readFileSync(file, "utf8")
		for (const id of [...Object.values(INTEREST_CONFIG_COLUMNS), ...Object.values(INTEREST_SIGNAL_WEIGHT_COLUMNS)])
			expect(src).toContain(`"${id}"`)
	})
	it("ask Postgres what the readers ask", () => {
		const file = join(here, "../../forYou/queries.ts")
		if (!existsSync(file)) return
		const src = readFileSync(file, "utf8")
		expect(src).toContain("(SELECT max_topics FROM personalization.for_you_config)")
		expect(src).toContain("personalization.feed_user_seen(")
	})
})

describe("readPrivateReadersShadowConfig", () => {
	it("is off by default and needs the private key path", () => {
		expect(readPrivateReadersShadowConfig({})).toMatchObject({enabled: false, keyPath: null})
		expect(
			readPrivateReadersShadowConfig({GEODB_PRIVATE_READERS_SHADOW: "1", GEODB_PRIVATE_KEY_PATH: " /k "}),
		).toMatchObject({enabled: true, keyPath: "/k"})
	})
})

describe("readTwinConfig", () => {
	it("reads the interest config, the six weights and max_topics", async () => {
		const c = await readTwinConfig(geoDbOf(), systemSpaceOf)
		expect(c.interest).toEqual({halfLifeDays: 30, spreadFraction: 0.5})
		expect(c.weights.size).toBe(INTEREST_SIGNAL_KINDS.length)
		expect(c.maxTopics).toBe(50)
	})
	it("is unsupported without the Space, a record, or with two values", async () => {
		await expect(readTwinConfig(geoDbOf(), async () => null)).rejects.toMatchObject({reason: "config_missing"})
		await expect(
			readTwinConfig(geoDbOf(configCards({[interestSignalWeightRecordId("vote")]: []})), systemSpaceOf),
		).rejects.toMatchObject({reason: "config_missing"})
		const twice = [row(INTEREST_CONFIG_COLUMNS.halfLifeDays, "30"), row(INTEREST_CONFIG_COLUMNS.halfLifeDays, "31")]
		await expect(
			readTwinConfig(geoDbOf(configCards({[interestConfigRecordId()]: twice})), systemSpaceOf),
		).rejects.toMatchObject({reason: "ambiguous_config"})
	})
})

describe("twinWeights", () => {
	const groupA = `28:${groupHex(USER)}`
	it("scans the user's group once and collection 23 once per own topic", async () => {
		const config = await readTwinConfig(geoDbOf(), systemSpaceOf)
		const reader = readerOf({
			[groupA]: {rows: [signalRow(TOPIC_A, "vote", 2)], more: false},
			[`23:${groupHex(TOPIC_A)}`]: {rows: [coocRow(TOPIC_A, TOPIC_B, 0.5)], more: false},
		})
		const rows = await twinWeights(reader, config, USER, AS_OF)
		expect(reader.scan).toHaveBeenCalledTimes(2)
		expect(rows.map((r) => r.topicId)).toEqual([TOPIC_A, TOPIC_B])
		expect(rows[0]?.weight).toBeCloseTo(2)
		expect(rows[1]).toMatchObject({topKind: "related", relatedViaTopicId: TOPIC_A})
		expect(rows[1]?.weight).toBeCloseTo(0.5)
	})
	it("names restricted, absent, truncated and too many topics", async () => {
		const config = await readTwinConfig(geoDbOf(), systemSpaceOf)
		await expect(twinWeights(readerOf({[groupA]: refused()}), config, USER, AS_OF)).rejects.toThrow()
		await expect(twinWeights(readerOf({}), config, USER, AS_OF)).rejects.toThrow()
		await expect(
			twinWeights(readerOf({[groupA]: {rows: [signalRow(TOPIC_A, "vote", 1)], more: true}}), config, USER, AS_OF),
		).rejects.toMatchObject({reason: "truncated"})
		const many = Array.from({length: MAX_SPREAD_SCANS + 1}, (_, i) =>
			signalRow(`00000000-0000-4000-8000-${(i + 1).toString(16).padStart(12, "0")}`, "vote", 1),
		)
		await expect(
			twinWeights(readerOf({[groupA]: {rows: many, more: false}}), config, USER, AS_OF),
		).rejects.toMatchObject({reason: "too_many_topics"})
	})
})

describe("twinSeen", () => {
	it("counts the window and calls a full page inside it truncated", async () => {
		const since = new Date(AS_OF.getTime() - 86400000)
		const recent = seenRow(new Date(AS_OF.getTime() - 3600000), 4)
		const counts = await twinSeen(readerOf({[`29:${groupHex(USER)}`]: {rows: [recent], more: false}}), USER, since)
		expect([...counts.values()]).toEqual([4])
		await expect(
			twinSeen(readerOf({[`29:${groupHex(USER)}`]: {rows: [recent], more: true}}), USER, since),
		).rejects.toMatchObject({reason: "truncated"})
	})
})

describe("comparisons", () => {
	it("match within tolerance and allow a swapped boundary tie", () => {
		expect(sameWeights([pgRow()], [pgRow({weight: 1 + 1e-12})])).toBe(true)
		expect(sameWeights([pgRow()], [pgRow({topKind: "comment"})])).toBe(false)
		expect(sameWeights([pgRow()], [pgRow({weight: 2})])).toBe(false)
		const tieA = [pgRow({topicId: TOPIC_A, weight: 2}), pgRow({topicId: TOPIC_B, weight: 1})]
		const tieB = [pgRow({topicId: TOPIC_A, weight: 2}), pgRow({topicId: ITEM, weight: 1})]
		expect(sameWeights(tieA, tieB)).toBe(true)
		expect(sameOwnWeights([{topicId: TOPIC_A, weight: 1}], [{topicId: TOPIC_A, weight: 1}])).toBe(true)
		expect(sameOwnWeights([{topicId: TOPIC_A, weight: 1}], [])).toBe(false)
		expect(sameCounts(new Map([[ITEM, 2]]), new Map([[ITEM, 2]]))).toBe(true)
		expect(sameCounts(new Map([[ITEM, 2]]), new Map([[ITEM, 3]]))).toBe(false)
	})
})

describe("createPrivateReadersShadow", () => {
	const db = {} as Database
	const log = {warn: vi.fn()}
	const reference = (over: Partial<ReferenceSide> = {}): ReferenceSide => ({
		sampleSignalsUser: async () => USER,
		sampleSeenUser: async () => USER,
		weights: async () => [pgRow({weight: 2, ownWeight: 2})],
		seen: async () => new Map([[ITEM, 4]]),
		...over,
	})
	const shadow = (reader: ReturnType<typeof readerOf>, ref = reference(), keyPath: string | null = "/k") =>
		createPrivateReadersShadow({
			config: {enabled: true, intervalMs: 1000, keyPath},
			db,
			reader,
			geoDb: geoDbOf(
				configCards({
					[interestConfigRecordId()]: [
						row(INTEREST_CONFIG_COLUMNS.halfLifeDays, "30"),
						row(INTEREST_CONFIG_COLUMNS.spreadFraction, "0"),
					],
				}),
			),
			systemSpaceOf,
			log,
			reference: ref,
			now: () => AS_OF,
		})
	const live = () =>
		readerOf({
			[`28:${groupHex(USER)}`]: {rows: [signalRow(TOPIC_A, "vote", 2)], more: false},
			[`29:${groupHex(USER)}`]: {rows: [seenRow(new Date(AS_OF.getTime() - 3600000), 4)], more: false},
		})

	it("exports nothing until mounted, then every series at zero", () => {
		expect(renderPrivateReadersShadowMetrics()).toBe("")
		shadow(live())
		expect(metric("weights", "match")).toBe(0)
		expect(metric("seen", "reference_failed")).toBe(0)
		expect(reasonMetric("busy")).toBe(0)
	})
	it("counts matches for all three readers", async () => {
		await shadow(live()).runOnce()
		expect(metric("weights", "match")).toBe(1)
		expect(metric("own_weights", "match")).toBe(1)
		expect(metric("seen", "match")).toBe(1)
	})
	it("counts a mismatch and logs no id or value", async () => {
		log.warn.mockClear()
		await shadow(live(), reference({seen: async () => new Map([[ITEM, 5]])})).runOnce()
		expect(metric("seen", "mismatch")).toBe(1)
		const logged = JSON.stringify(log.warn.mock.calls)
		for (const secret of [USER, ITEM, TOPIC_A, "5"]) expect(logged).not.toContain(secret)
	})
	it("counts restricted and absent apart", async () => {
		await shadow(readerOf({[`28:${groupHex(USER)}`]: refused(), [`29:${groupHex(USER)}`]: refused()})).runOnce()
		expect(metric("weights", "restricted")).toBe(1)
		expect(metric("own_weights", "restricted")).toBe(1)
		expect(metric("seen", "restricted")).toBe(1)
		await shadow(readerOf({})).runOnce()
		expect(metric("weights", "absent")).toBe(1)
		expect(metric("seen", "absent")).toBe(1)
	})
	it("counts the reference side and geo-db failures", async () => {
		const timeout = Object.assign(new Error("canceling statement"), {code: "57014"})
		await shadow(live(), reference({weights: async () => Promise.reject(timeout)})).runOnce()
		expect(metric("weights", "reference_timeout")).toBe(1)
		await shadow(readerOf({[`29:${groupHex(USER)}`]: new GeoDbError("timeout", "slow")})).runOnce()
		expect(metric("seen", "failed")).toBe(1)
	})
	it("does nothing when no user has rows, and counts busy for every reader", async () => {
		const s = shadow(live(), reference({sampleSignalsUser: async () => null, sampleSeenUser: async () => null}))
		await s.runOnce()
		expect(renderPrivateReadersShadowMetrics()).not.toMatch(/\} [1-9]/)
		let release: (u: string) => void = () => {}
		const gate = new Promise<string>((r) => {
			release = r
		})
		const slow = shadow(live(), reference({sampleSignalsUser: () => gate}))
		const first = slow.runOnce()
		await slow.runOnce()
		expect(reasonMetric("busy")).toBe(1)
		expect(metric("seen", "unsupported")).toBe(1)
		release(USER)
		await first
	})
	it("does not start without the private key path", () => {
		expect(shadow(live(), reference(), null).start()).toBeTypeOf("function")
	})
})
