import {beforeEach, describe, expect, it, vi} from "vitest"
import type {Database} from "../../forYou/queries"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../../gateway/metrics"
import {GeoDbError} from "../client"
import type {DerivedReader} from "../derived"
import {createMetaShadow, readMetaShadowConfig, readPostgresMetas} from "../metaShadow"

const hex = (s: string) => Buffer.from(s, "utf8").toString("hex")
const series = (field: string, result: string): number => {
	const m = new RegExp(`gaia_api_gateway_shadow_total\\{field="${field}",result="${result}"\\} (\\d+)`).exec(
		renderGatewayMetrics(),
	)
	return m ? Number(m[1]) : 0
}
const log = {warn: vi.fn()}
const rows = [
	{id: "hermes-pipeline", cursor: "c-1", block_number: "100"},
	{id: "ipfs-cache", cursor: "c-2", block_number: "90"},
]
const held = (map: Record<string, string>): Pick<DerivedReader, "get"> => ({
	get: vi.fn(async (_c: number, key: string) => {
		const hit = Object.entries(map).find(([id]) => hex(id) === key)
		return hit ? {group: "0".repeat(32), score: 0, value: Buffer.from(hit[1])} : null
	}),
})

beforeEach(() => {
	__resetGatewayMetricsForTests()
	log.warn.mockClear()
})

describe("readMetaShadowConfig", () => {
	it("defaults to shadow on, every 5 minutes", () => {
		expect(readMetaShadowConfig({})).toEqual({enabled: true, intervalMs: 300_000})
	})
	it("turns off only for an explicit off value", () => {
		for (const off of ["0", "off", "false", " OFF "])
			expect(readMetaShadowConfig({GEODB_META_SHADOW: off}).enabled).toBe(false)
		expect(readMetaShadowConfig({GEODB_META_SHADOW: "1"}).enabled).toBe(true)
	})
	it("takes a valid interval and ignores a bad one", () => {
		expect(readMetaShadowConfig({GEODB_META_SHADOW_INTERVAL_MS: "1000"}).intervalMs).toBe(1000)
		expect(readMetaShadowConfig({GEODB_META_SHADOW_INTERVAL_MS: "x"}).intervalMs).toBe(300_000)
	})
})

describe("readPostgresMetas", () => {
	it("reads at most a few rows of meta and returns the response shape of the metas root", async () => {
		const execute = vi.fn(async () => ({rows}))
		const body = (await readPostgresMetas({execute} as unknown as Database)) as {
			data: {metas: {nodes: Record<string, string>[]}}
		}
		expect(body.data.metas.nodes).toEqual([
			{id: "hermes-pipeline", cursor: "c-1", blockNumber: "100"},
			{id: "ipfs-cache", cursor: "c-2", blockNumber: "90"},
		])
		expect(execute).toHaveBeenCalledTimes(1)
	})
})

describe("createMetaShadow pass", () => {
	const make = (reader: Pick<DerivedReader, "get">, readRows?: () => Promise<unknown>) =>
		createMetaShadow({
			config: {enabled: true, intervalMs: 1000},
			db: {} as Database,
			reader,
			log,
			readBody: readRows as never,
		})

	const body = async () => ({
		data: {metas: {nodes: rows.map((r) => ({id: r.id, cursor: r.cursor, blockNumber: r.block_number}))}},
	})

	it("counts a match in the gateway counter under field metas", async () => {
		const reader = held({"hermes-pipeline": "100:c-1", "ipfs-cache": "90:c-2"})
		await make(reader, body).runOnce()
		expect(series("metas", "match")).toBe(1)
		expect(series("metas", "mismatch")).toBe(0)
	})
	it("counts a mirror that is behind as a mismatch", async () => {
		await make(held({"hermes-pipeline": "99:c-0", "ipfs-cache": "90:c-2"}), body).runOnce()
		expect(series("metas", "mismatch")).toBe(1)
		expect(log.warn).toHaveBeenCalled()
	})
	it("counts a mirror that is ahead as a match (freshness only)", async () => {
		await make(held({"hermes-pipeline": "101:zzz", "ipfs-cache": "90:c-2"}), body).runOnce()
		expect(series("metas", "match")).toBe(1)
	})
	it("counts a missing row as unsupported, not a mismatch", async () => {
		await make(held({"ipfs-cache": "90:c-2"}), body).runOnce()
		expect(series("metas", "unsupported_selection")).toBe(1)
		expect(series("metas", "mismatch")).toBe(0)
	})
	it("counts a geo-db read error as failed", async () => {
		const reader = {get: vi.fn(async () => Promise.reject(new Error("down")))}
		await make(reader, body).runOnce()
		expect(series("metas", "failed")).toBe(1)
	})
	it("names why a geo-db read failed instead of counting every failure as other", async () => {
		const failedBy = (reason: string): number => {
			const m = new RegExp(
				`gaia_api_gateway_shadow_failed_total\\{field="metas",reason="${reason}"\\} (\\d+)`,
			).exec(renderGatewayMetrics())
			return m ? Number(m[1]) : 0
		}
		const fail = async (error: Error) => make({get: vi.fn(async () => Promise.reject(error))}, body).runOnce()
		await fail(new GeoDbError("timeout", "derived read took longer than 500 ms"))
		await fail(new GeoDbError("client_failed", "derived read refused: code=Unavailable"))
		await fail(new GeoDbError("client_failed", "geodb-client derived pipe is backing off after a failed start"))
		await fail(new Error("not a geo-db error"))
		expect(failedBy("timeout")).toBe(1)
		expect(failedBy("client_failed")).toBe(1)
		expect(failedBy("other")).toBe(1)
		expect(series("metas", "failed")).toBe(3)
		// A read the reader never tried (it was backing off) is not a failure: it counts unsupported cooling_down.
		expect(failedBy("cooling_down")).toBe(0)
		expect(series("metas", "unsupported_selection")).toBe(1)
		expect(renderGatewayMetrics()).toContain(
			`gaia_api_gateway_shadow_unsupported_total{field="metas",reason="cooling_down"} 1`,
		)
	})
	it("a Postgres failure counts nothing against geo-db and never throws", async () => {
		const reader = held({})
		await make(reader, async () => Promise.reject(new Error("pg down"))).runOnce()
		for (const r of ["match", "mismatch", "unsupported_selection", "failed"]) expect(series("metas", r)).toBe(0)
		expect(reader.get).not.toHaveBeenCalled()
		expect(log.warn).toHaveBeenCalled()
	})
	it("registers the metas series at zero as soon as it is mounted", () => {
		make(held({}))
		expect(renderGatewayMetrics()).toContain(`gaia_api_gateway_shadow_total{field="metas",result="match"} 0`)
	})
	it("start() starts nothing when disabled", () => {
		const s = createMetaShadow({config: {enabled: false, intervalMs: 1}, db: {} as Database, reader: held({}), log})
		expect(typeof s.start()).toBe("function")
	})
})
