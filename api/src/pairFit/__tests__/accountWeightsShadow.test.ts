import {beforeEach, describe, expect, it, vi} from "vitest"
import {GeoDbError} from "../../geodb/client"
import type {DerivedReader, DerivedRow} from "../../geodb/derived"
import {
	__resetAccountWeightsMetricsForTests,
	ACCOUNT_EXCLUSIONS_COLLECTION,
	ACCOUNT_EXCLUSIONS_VALUE_BYTES,
	ACCOUNT_WEIGHTS_COLLECTION,
	ACCOUNT_WEIGHTS_VALUE_BYTES,
	type AccountWeightsShadowConfig,
	compareAccountExclusion,
	compareAccountWeight,
	createAccountWeightsSampler,
	createAccountWeightsShadow,
	readAccountWeightsShadowConfig,
	renderAccountWeightsMetrics,
} from "../accountWeightsShadow"

const U1 = "11111111111141118111111111111111"
const U2 = "22222222222242228222222222222222"
const value = Buffer.alloc(ACCOUNT_WEIGHTS_VALUE_BYTES)
const row = (score: number, v = value): DerivedRow => ({group: "0".repeat(32), score, value: v})
const config: AccountWeightsShadowConfig = {
	enabled: true,
	maxInFlight: 2,
	intervalMs: 1000,
	sample: 5,
	exclusions: false,
	pollMs: 1000,
}
const log = {warn: vi.fn()}
const settle = () => new Promise((r) => setTimeout(r, 5))
const count = (result: string) =>
	Number(
		new RegExp(`gaia_api_account_weights_shadow_total\\{result="${result}"\\} (\\d+)`).exec(
			renderAccountWeightsMetrics(),
		)?.[1],
	)

beforeEach(() => {
	__resetAccountWeightsMetricsForTests()
	log.warn.mockClear()
})

describe("config", () => {
	it("is off unless the flag says on", () => {
		expect(readAccountWeightsShadowConfig({}).enabled).toBe(false)
		expect(readAccountWeightsShadowConfig({GEODB_ACCOUNT_WEIGHTS_SHADOW: "0"}).enabled).toBe(false)
		expect(readAccountWeightsShadowConfig({GEODB_ACCOUNT_WEIGHTS_SHADOW: "1"}).enabled).toBe(true)
		const c = readAccountWeightsShadowConfig({
			GEODB_ACCOUNT_WEIGHTS_SHADOW_SAMPLE: "999",
			GEODB_ACCOUNT_WEIGHTS_SHADOW_INTERVAL_MS: "x",
		})
		expect(c).toMatchObject({sample: 50, intervalMs: 10_000, maxInFlight: 1})
	})
})

describe("compareAccountWeight", () => {
	it("matches equal weights and a missing row on both sides", () => {
		expect(compareAccountWeight(0.5, row(0.5))).toBe("match")
		expect(compareAccountWeight(null, null)).toBe("match")
	})
	it("flags a different weight, and a row Postgres lacks", () => {
		expect(compareAccountWeight(0.5, row(0.6))).toBe("mismatch")
		expect(compareAccountWeight(null, row(0.6))).toBe("mismatch")
	})
	it("calls a missing geo-db row or another layout unsupported", () => {
		expect(compareAccountWeight(0.5, null)).toBe("row_missing")
		expect(compareAccountWeight(0.5, row(0.5, Buffer.alloc(3)))).toBe("bad_layout")
	})
})

describe("createAccountWeightsShadow", () => {
	it("exports nothing until mounted, then every series at zero", () => {
		expect(renderAccountWeightsMetrics()).toBe("")
		createAccountWeightsShadow({config, reader: {get: vi.fn(), close: vi.fn()}, log})
		expect(count("match")).toBe(0)
		expect(renderAccountWeightsMetrics()).toContain('reason="row_missing"')
	})

	it("reads collection 5 by user id and counts each outcome", async () => {
		const get = vi.fn(async (_c: number, key: string) => (key === U1 ? row(0.5) : key === U2 ? row(0.9) : null))
		const shadow = createAccountWeightsShadow({config, reader: {get, close: vi.fn()}, log})
		shadow.compare(
			new Map([
				[U1, {weight: 0.5}],
				[U2, {weight: 0.1}],
				["33333333333343338333333333333333", {weight: 0.2}],
			]),
		)
		await settle()
		expect(get).toHaveBeenCalledWith(ACCOUNT_WEIGHTS_COLLECTION, U1)
		expect([count("match"), count("mismatch"), count("unsupported")]).toEqual([1, 1, 1])
		expect(renderAccountWeightsMetrics()).toContain('unsupported_total{reason="row_missing"} 1')
		expect(log.warn).toHaveBeenCalledOnce()
		expect(JSON.stringify(log.warn.mock.calls)).not.toContain(U2)
	})

	it("counts a failed read and never throws", async () => {
		const reader: DerivedReader = {
			get: vi.fn().mockRejectedValue(new GeoDbError("timeout", "slow")),
			close: vi.fn(),
		}
		createAccountWeightsShadow({config, reader, log}).compare(new Map([[U1, {weight: 0.5}]]))
		await settle()
		expect(count("failed")).toBe(1)
	})

	it("does nothing when off, and compares only a sample", async () => {
		const get = vi.fn(async () => null)
		createAccountWeightsShadow({config: {...config, enabled: false}, reader: {get, close: vi.fn()}, log}).compare(
			new Map([[U1, {weight: null}]]),
		)
		await settle()
		expect(get).not.toHaveBeenCalled()
		const many = new Map(Array.from({length: 9}, (_, i) => [`${i}`.repeat(32), {weight: null}] as const))
		createAccountWeightsShadow({config: {...config, sample: 3}, reader: {get, close: vi.fn()}, log}).compare(many)
		await settle()
		expect(get).toHaveBeenCalledTimes(3)
	})

	it("throttles by interval and counts a busy skip", async () => {
		let t = 0
		let release: (r: DerivedRow | null) => void = () => {}
		const get = vi.fn(
			() =>
				new Promise<DerivedRow | null>((r) => {
					release = r
				}),
		)
		const shadow = createAccountWeightsShadow({
			config: {...config, maxInFlight: 1},
			reader: {get, close: vi.fn()},
			log,
			now: () => t,
		})
		const one = new Map([[U1, {weight: null}]])
		shadow.compare(one)
		shadow.compare(one) // inside the interval
		expect(get).toHaveBeenCalledOnce()
		t = 2000
		shadow.compare(one) // past the interval, the first read is still out
		expect(renderAccountWeightsMetrics()).toContain('unsupported_total{reason="busy"} 1')
		release(null)
		await settle()
		expect(count("match")).toBe(1)
	})
})

describe("account exclusions", () => {
	const exConfig = {...config, exclusions: true}
	const exCount = (result: string) =>
		Number(
			new RegExp(`gaia_api_account_exclusions_shadow_total\\{result="${result}"\\} (\\d+)`).exec(
				renderAccountWeightsMetrics(),
			)?.[1],
		)
	const held = (bytes = ACCOUNT_EXCLUSIONS_VALUE_BYTES) => ({
		group: "0".repeat(32),
		score: 0,
		value: Buffer.alloc(bytes),
	})

	it("compares excluded with the presence of a collection 12 row", () => {
		expect(compareAccountExclusion(true, held())).toBe("match")
		expect(compareAccountExclusion(false, null)).toBe("match")
		expect(compareAccountExclusion(false, held())).toBe("mismatch")
		expect(compareAccountExclusion(true, null)).toBe("unsupported")
		expect(compareAccountExclusion(true, held(4))).toBe("unsupported")
	})

	it("reads collection 12 only when the second flag is on", async () => {
		const get = vi.fn(async () => null)
		createAccountWeightsShadow({config, reader: {get, close: vi.fn()}, log}).compare(
			new Map([[U1, {weight: 0.5, excluded: false}]]),
		)
		await settle()
		expect(get).not.toHaveBeenCalledWith(ACCOUNT_EXCLUSIONS_COLLECTION, expect.anything())
		expect(renderAccountWeightsMetrics()).not.toContain("account_exclusions_shadow")
	})

	it("counts each outcome beside the weights", async () => {
		const get = vi.fn(async (c: number, key: string) =>
			c === ACCOUNT_EXCLUSIONS_COLLECTION ? (key === U1 ? held() : null) : null,
		)
		const shadow = createAccountWeightsShadow({config: exConfig, reader: {get, close: vi.fn()}, log})
		shadow.compare(
			new Map([
				[U1, {weight: 0, excluded: true}],
				[U2, {weight: 0.4, excluded: false}],
				["33333333333343338333333333333333", {weight: 0, excluded: true}],
			]),
		)
		await settle()
		expect(get).toHaveBeenCalledWith(ACCOUNT_EXCLUSIONS_COLLECTION, U1)
		expect([exCount("match"), exCount("mismatch"), exCount("unsupported"), exCount("failed")]).toEqual([2, 0, 1, 0])
	})

	it("counts a failed collection 12 read", async () => {
		const get = vi.fn(async (c: number) => {
			if (c === ACCOUNT_EXCLUSIONS_COLLECTION) throw new GeoDbError("timeout", "slow")
			return null
		})
		createAccountWeightsShadow({config: exConfig, reader: {get, close: vi.fn()}, log}).compare(
			new Map([[U1, {weight: 0.5, excluded: false}]]),
		)
		await settle()
		expect(exCount("failed")).toBe(1)
	})

	it("reads the flag", () => {
		expect(readAccountWeightsShadowConfig({}).exclusions).toBe(false)
		expect(readAccountWeightsShadowConfig({GEODB_ACCOUNT_EXCLUSIONS_SHADOW: "1"}).exclusions).toBe(true)
	})
})

describe("the sampler", () => {
	const dashed = (hex: string) =>
		`${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
	// Answers the id sample first, then the readAccounts join: the two statements the sampler issues.
	const fakeDb = (ids: string[], accounts: {user_id: string; weight: string | null; excluded: boolean}[]) => {
		let calls = 0
		return {execute: vi.fn(async () => ({rows: ++calls === 1 ? ids.map((user_id) => ({user_id})) : accounts}))}
	}

	it("compares sampled Postgres accounts with no pair-fit request", async () => {
		const reader = {get: vi.fn(async () => row(0.5)), scan: vi.fn()} as unknown as DerivedReader
		const shadow = createAccountWeightsShadow({config, reader, log})
		const db = fakeDb([dashed(U1)], [{user_id: dashed(U1), weight: "0.5", excluded: false}])
		await createAccountWeightsSampler({
			config: {...config, sample: 1},
			db: db as never,
			shadow,
			log,
			random: () => dashed(U2),
		}).runOnce()
		await settle()
		expect(db.execute).toHaveBeenCalledTimes(2)
		expect(reader.get).toHaveBeenCalledWith(ACCOUNT_WEIGHTS_COLLECTION, U1)
		expect(count("match")).toBe(1)
	})

	it("swallows a Postgres failure and stays quiet when the table is empty", async () => {
		const shadow = {compare: vi.fn()}
		const down = {execute: vi.fn().mockRejectedValue(new Error("pg down"))}
		await createAccountWeightsSampler({config, db: down as never, shadow, log}).runOnce()
		expect(log.warn).toHaveBeenCalled()
		await createAccountWeightsSampler({config, db: fakeDb([], []) as never, shadow, log}).runOnce()
		expect(shadow.compare).not.toHaveBeenCalled()
	})

	it("is inert while the flag is off", () => {
		const shadow = {compare: vi.fn()}
		const stop = createAccountWeightsSampler({
			config: {...config, enabled: false},
			db: fakeDb([], []) as never,
			shadow,
			log,
		}).start()
		stop()
		expect(shadow.compare).not.toHaveBeenCalled()
	})
})
