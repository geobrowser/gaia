import {beforeEach, describe, expect, it, vi} from "vitest"
import {GeoDbError} from "../client"
import {__resetGeoDbMetricsForTests, activateGeoDbMetrics, renderGeoDbCardMetrics} from "../metrics"
import {type CardDeps, resolveCard} from "../service"

const ENTITY = "073ff16be7fe4591a984c325d629d92d"
const SPACE = "11111111111141118111111111111111"
const GEO_SPACE = "22222222222242228222222222222222"
const PROP = "98012398829d48b083738f2589675894"

const pgRows = {
	values: [
		{
			property_id: PROP,
			text: "Noam",
			integer: null,
			float: null,
			decimal: null,
			boolean: null,
			datetime: null,
			date: null,
		},
	],
	relations: [],
}

function deps(mode: "on" | "shadow", readCard: () => Promise<{prop: string; raw: string}[]>): CardDeps {
	return {
		mode,
		geoDb: {readCard},
		geoSpaceOf: async () => GEO_SPACE,
		readPostgres: async () => pgRows,
		log: {info: vi.fn(), warn: vi.fn()},
	}
}

const line = (text: string, series: string) =>
	text
		.split("\n")
		.find((l) => l.startsWith(`${series} `))
		?.split(" ")
		.pop()

describe("geo-db card metrics", () => {
	beforeEach(() => __resetGeoDbMetricsForTests())

	it("exports nothing until the route is mounted", () => {
		expect(renderGeoDbCardMetrics()).toBe("")
	})

	it("registers every series at zero once active", () => {
		activateGeoDbMetrics()
		const text = renderGeoDbCardMetrics()
		expect(line(text, 'gaia_api_geodb_card_fallback_total{reason="timeout"}')).toBe("0")
		expect(line(text, 'gaia_api_geodb_card_shadow_total{result="match"}')).toBe("0")
		expect(line(text, 'gaia_api_geodb_read_seconds_bucket{outcome="ok",le="+Inf"}')).toBe("0")
		expect(line(text, 'gaia_api_postgres_card_read_seconds_bucket{outcome="ok",le="+Inf"}')).toBe("0")
	})

	it("times the Postgres card read in shadow mode only, not the mode-on fallback", async () => {
		activateGeoDbMetrics()
		await resolveCard(
			deps("shadow", async () => [{prop: PROP, raw: "Noam"}]),
			ENTITY,
			SPACE,
		)
		await resolveCard(
			deps("on", async () => {
				throw new GeoDbError("timeout", "slow")
			}),
			ENTITY,
			SPACE,
		)
		const text = renderGeoDbCardMetrics()
		expect(line(text, 'gaia_api_postgres_card_read_seconds_count{outcome="ok"}')).toBe("1")
		expect(line(text, 'gaia_api_postgres_card_read_seconds_count{outcome="error"}')).toBe("0")
	})

	it("counts geo-db answers, fallbacks by reason, and the read latency", async () => {
		activateGeoDbMetrics()
		await resolveCard(
			deps("on", async () => [{prop: PROP, raw: "Noam"}]),
			ENTITY,
			SPACE,
		)
		await resolveCard(
			deps("on", async () => {
				throw new GeoDbError("timeout", "slow")
			}),
			ENTITY,
			SPACE,
		)
		const text = renderGeoDbCardMetrics()
		expect(line(text, 'gaia_api_geodb_card_served_total{source="geodb"}')).toBe("1")
		expect(line(text, 'gaia_api_geodb_card_served_total{source="postgres"}')).toBe("1")
		expect(line(text, 'gaia_api_geodb_card_fallback_total{reason="timeout"}')).toBe("1")
		expect(line(text, 'gaia_api_geodb_read_seconds_count{outcome="ok"}')).toBe("1")
		expect(line(text, 'gaia_api_geodb_read_seconds_count{outcome="error"}')).toBe("1")
	})

	it("classifies shadow results", async () => {
		activateGeoDbMetrics()
		const run = async (read: () => Promise<{prop: string; raw: string}[]>) =>
			(await resolveCard(deps("shadow", read), ENTITY, SPACE)).shadow
		await run(async () => [{prop: PROP, raw: "Noam"}])
		await run(async () => [{prop: PROP, raw: "Noam S."}])
		await run(async () => [])
		await run(async () => [
			{prop: PROP, raw: "Noam"},
			{prop: PROP, raw: "Extra"},
		])
		await run(async () => {
			throw new GeoDbError("client_failed", "down")
		})
		const text = renderGeoDbCardMetrics()
		for (const result of ["match", "both", "only_postgres", "only_geodb", "failed"]) {
			expect(line(text, `gaia_api_geodb_card_shadow_total{result="${result}"}`)).toBe("1")
		}
	})
})
