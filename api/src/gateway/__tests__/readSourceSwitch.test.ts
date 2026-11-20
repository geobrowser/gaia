import {mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import path from "node:path"
import {afterEach, beforeEach, describe, expect, it, vi} from "vitest"
import {GeoDbError} from "../../geodb/client"
import {__resetGeoDbMetricsForTests} from "../../geodb/metrics"
import {type CardDeps, resolveCard} from "../../geodb/service"
import {readGatewayConfig} from "../config"
import {createGateway, type FieldResolver} from "../gateway"
import {
	__resetReadSourceSwitchMetricsForTests,
	createReadSourceSwitch,
	DEFAULT_AUTO_REVERT_CONFIG,
	ENTITY_CARDS_FIELD,
	isGeoDbErrorReason,
	renderReadSourceSwitchMetrics,
} from "../readSourceSwitch"
import {__resetServeMetricsForTests} from "../serveMetrics"

const json = (body: unknown) =>
	new Request("http://api/graphql", {
		method: "POST",
		headers: {"content-type": "application/json"},
		body: JSON.stringify(body),
	})

const metric = (name: string, labels: string) =>
	Number(
		renderReadSourceSwitchMetrics()
			.split("\n")
			.find((line) => line.startsWith(`${name}{${labels}}`))
			?.split(" ")[1] ?? "0",
	)

describe("readSourceSwitch", () => {
	let dir: string
	let filePath: string

	beforeEach(() => {
		dir = mkdtempSync(path.join(tmpdir(), "read-source-switch-"))
		filePath = path.join(dir, "read-source.json")
		__resetReadSourceSwitchMetricsForTests()
		__resetServeMetricsForTests()
		__resetGeoDbMetricsForTests()
	})

	afterEach(() => {
		rmSync(dir, {recursive: true, force: true})
	})

	const writeConfig = (payload: unknown, bumpMs = 0) => {
		writeFileSync(filePath, typeof payload === "string" ? payload : JSON.stringify(payload), "utf8")
		if (bumpMs !== 0) {
			const t = (Date.now() + bumpMs) / 1000
			utimesSync(filePath, t, t)
		}
	}

	it("reads off, shadow, and on modes at runtime from the watched ConfigMap file without restart", async () => {
		let now = 1_000
		writeConfig({fields: {entity: "shadow", entities: "off", [ENTITY_CARDS_FIELD]: "shadow"}})
		const sw = createReadSourceSwitch({
			path: filePath,
			defaults: new Map([
				["entity", "shadow"],
				["relations", "shadow"],
				[ENTITY_CARDS_FIELD, "shadow"],
			]),
			refreshMs: 2_000,
			now: () => now,
		})

		await sw.refreshIfDue()
		expect(sw.effectiveMode("entity")).toBe("shadow")
		expect(sw.effectiveMode("entities")).toBe("off")
		// Unmentioned field keeps its startup default
		expect(sw.effectiveMode("relations")).toBe("shadow")
		expect(sw.effectiveMode(ENTITY_CARDS_FIELD)).toBe("shadow")

		// Flip entity to on and entityCards to on; before refreshMs nothing reloads
		writeConfig({fields: {entity: "on", entities: "shadow", [ENTITY_CARDS_FIELD]: "on"}}, 5_000)
		now += 1_000
		await sw.refreshIfDue()
		expect(sw.effectiveMode("entity")).toBe("shadow")

		// After refreshMs the new modes take effect immediately
		now += 1_500
		await sw.refreshIfDue()
		expect(sw.effectiveMode("entity")).toBe("on")
		expect(sw.effectiveMode("entities")).toBe("shadow")
		expect(sw.effectiveMode(ENTITY_CARDS_FIELD)).toBe("on")

		// Flip back to off
		writeConfig({fields: {entity: "off", [ENTITY_CARDS_FIELD]: "off"}}, 10_000)
		now += 2_500
		await sw.refreshIfDue()
		expect(sw.effectiveMode("entity")).toBe("off")
		expect(sw.effectiveMode(ENTITY_CARDS_FIELD)).toBe("off")
	})

	it("keeps startup defaults when file is absent and keeps last good config on malformed JSON", async () => {
		let now = 1_000
		const sw = createReadSourceSwitch({
			path: filePath,
			defaults: new Map([["entity", "shadow"]]),
			refreshMs: 1_000,
			now: () => now,
		})

		// File does not exist yet
		await sw.refreshIfDue()
		expect(sw.effectiveMode("entity")).toBe("shadow")

		// Valid file appears
		writeConfig({fields: {entity: "on"}}, 2_000)
		now += 2_000
		await sw.refreshIfDue()
		expect(sw.effectiveMode("entity")).toBe("on")

		// Corrupted file keeps last known good value
		writeConfig("{not valid json", 4_000)
		now += 2_000
		await sw.refreshIfDue()
		expect(sw.effectiveMode("entity")).toBe("on")
	})

	it("auto-reverts a field when geo-db error rate crosses the threshold over a sliding window, logs once, bumps counter once, and stays reverted until reset", async () => {
		let now = 10_000
		const warn = vi.fn()
		writeConfig({fields: {entity: "on"}})
		const sw = createReadSourceSwitch({
			path: filePath,
			defaults: new Map([["entity", "shadow"]]),
			refreshMs: 1_000,
			now: () => now,
			log: {info: vi.fn(), warn},
			autoRevert: {windowMs: 60_000, minSamples: 4, maxErrorRate: 0.25, maxLatencyMs: 250},
		})
		await sw.refreshIfDue()
		expect(sw.effectiveMode("entity")).toBe("on")

		// 3 served, 1 failure => 25% (at threshold, does not trip)
		sw.recordOutcome("entity", {kind: "served", latencyMs: 10})
		sw.recordOutcome("entity", {kind: "served", latencyMs: 10})
		sw.recordOutcome("entity", {kind: "served", latencyMs: 10})
		sw.recordOutcome("entity", {kind: "failure", latencyMs: 10})
		expect(sw.effectiveMode("entity")).toBe("on")
		expect(sw.isReverted("entity")).toBe(false)
		expect(warn).not.toHaveBeenCalled()

		// 1 more failure => 2/5 = 40% > 25% => trips!
		sw.recordOutcome("entity", {kind: "failure", latencyMs: 10})
		expect(sw.isReverted("entity")).toBe(true)
		expect(sw.effectiveMode("entity")).toBe("off")
		expect(warn).toHaveBeenCalledTimes(1)
		expect(metric("gaia_api_gateway_read_source_auto_revert_total", 'field="entity",reason="error_rate"')).toBe(1)

		// Even after the sliding window expires and file still says "on", it stays reverted and does not re-log or re-bump
		now += 120_000
		await sw.refreshIfDue()
		sw.recordOutcome("entity", {kind: "failure", latencyMs: 10})
		expect(sw.isReverted("entity")).toBe(true)
		expect(sw.effectiveMode("entity")).toBe("off")
		expect(warn).toHaveBeenCalledTimes(1)
		expect(metric("gaia_api_gateway_read_source_auto_revert_total", 'field="entity",reason="error_rate"')).toBe(1)

		// Human resets via ConfigMap reset token
		writeConfig({fields: {entity: "on"}, reset: {entity: "2026-11-20T03:00:00Z"}}, 10_000)
		now += 2_000
		await sw.refreshIfDue()
		expect(sw.isReverted("entity")).toBe(false)
		expect(sw.effectiveMode("entity")).toBe("on")

		// Same reset token does not clear a second trip; a new token does
		for (let i = 0; i < 4; i++) sw.recordOutcome("entity", {kind: "failure", latencyMs: 10})
		expect(sw.isReverted("entity")).toBe(true)
		expect(warn).toHaveBeenCalledTimes(2)
		expect(metric("gaia_api_gateway_read_source_auto_revert_total", 'field="entity",reason="error_rate"')).toBe(2)

		now += 2_000
		await sw.refreshIfDue()
		expect(sw.isReverted("entity")).toBe(true)

		writeConfig({fields: {entity: "on"}, reset: {entity: "2026-11-20T03:05:00Z"}}, 20_000)
		now += 2_000
		await sw.refreshIfDue()
		expect(sw.isReverted("entity")).toBe(false)
		expect(sw.effectiveMode("entity")).toBe("on")
	})

	it("auto-reverts a field when geo-db latency crosses the threshold over a sliding window and logs/counts once", async () => {
		let now = 10_000
		const warn = vi.fn()
		writeConfig({fields: {entities: "on"}})
		const sw = createReadSourceSwitch({
			path: filePath,
			defaults: new Map([["entities", "shadow"]]),
			refreshMs: 1_000,
			now: () => now,
			log: {info: vi.fn(), warn},
			autoRevert: {windowMs: 60_000, minSamples: 4, maxErrorRate: 0.5, maxLatencyMs: 100},
		})
		await sw.refreshIfDue()

		// Old slow samples age out of the sliding window before minSamples is reached
		sw.recordOutcome("entities", {kind: "served", latencyMs: 500})
		sw.recordOutcome("entities", {kind: "served", latencyMs: 500})
		now += 70_000
		sw.recordOutcome("entities", {kind: "served", latencyMs: 20})
		sw.recordOutcome("entities", {kind: "served", latencyMs: 20})
		sw.recordOutcome("entities", {kind: "served", latencyMs: 20})
		sw.recordOutcome("entities", {kind: "served", latencyMs: 20})
		expect(sw.isReverted("entities")).toBe(false)

		// Now 4 slow samples in the active window trip latency auto-revert
		now += 70_000
		for (let i = 0; i < 4; i++) sw.recordOutcome("entities", {kind: "served", latencyMs: 180})
		expect(sw.isReverted("entities")).toBe(true)
		expect(sw.effectiveMode("entities")).toBe("off")
		expect(warn).toHaveBeenCalledTimes(1)
		expect(metric("gaia_api_gateway_read_source_auto_revert_total", 'field="entities",reason="latency"')).toBe(1)

		// Programmatic reset clears the latch
		sw.reset("entities")
		expect(sw.isReverted("entities")).toBe(false)
		expect(sw.effectiveMode("entities")).toBe("on")
	})

	it("integrates with createGateway across shadow, on, auto-revert, reset, and off without counting translator declines as errors", async () => {
		let now = 1_000
		const warn = vi.fn()
		const query = {query: "query Q { entity(id: 1) { id } }", operationName: "Q"}
		writeConfig({fields: {entity: "shadow"}})
		const sw = createReadSourceSwitch({
			path: filePath,
			defaults: new Map([["entity", "shadow"]]),
			refreshMs: 1_000,
			now: () => now,
			log: {info: vi.fn(), warn},
			autoRevert: {windowMs: 60_000, minSamples: 3, maxErrorRate: 0.25, maxLatencyMs: 250},
		})

		const delegate = vi.fn(
			async () =>
				new Response(JSON.stringify({data: {entity: {id: "pg"}}}), {
					headers: {"content-type": "application/json"},
				}),
		)
		const shadow = vi.fn(async () => {})
		let resolverImpl: FieldResolver = async () => ({ok: true, data: {entity: {id: "geo"}}})
		const resolver: FieldResolver = (ctx) => resolverImpl(ctx)

		const gateway = createGateway({
			config: readGatewayConfig({
				GRAPHQL_GATEWAY: "on",
				GRAPHQL_GATEWAY_FIELDS: "entity=shadow",
				GRAPHQL_GATEWAY_SHADOW_SAMPLE: "1",
			}),
			delegate,
			resolvers: new Map([["entity", resolver]]),
			shadows: new Map([["entity", shadow]]),
			readSourceSwitch: sw,
			random: () => 0,
		})

		// 1. Starts in shadow: PostGraphile answers, shadow runs
		expect(await (await gateway(json(query), {})).json()).toEqual({data: {entity: {id: "pg"}}})
		await new Promise((r) => setTimeout(r, 10))
		expect(shadow).toHaveBeenCalledTimes(1)

		// 2. Flip to on at runtime: geo-db resolver answers
		writeConfig({fields: {entity: "on"}}, 2_000)
		now += 2_000
		expect(await (await gateway(json(query), {})).json()).toEqual({data: {entity: {id: "geo"}}})

		// Translator declines (e.g. "selection") fall back to Postgres but do NOT trigger auto-revert
		resolverImpl = async () => ({ok: false, reason: "selection"})
		for (let i = 0; i < 4; i++) {
			expect(await (await gateway(json(query), {})).json()).toEqual({data: {entity: {id: "pg"}}})
		}
		expect(sw.isReverted("entity")).toBe(false)

		// 3. Geo-db errors trip auto-revert -> falls back to Postgres and stops calling resolver
		const spyResolver = vi.fn(async () => ({ok: false as const, reason: "timeout"}))
		resolverImpl = spyResolver
		for (let i = 0; i < 3; i++) {
			await gateway(json(query), {})
		}
		expect(sw.isReverted("entity")).toBe(true)
		expect(warn).toHaveBeenCalledTimes(1)

		// Next request goes straight to Postgres without calling geo-db resolver
		spyResolver.mockClear()
		expect(await (await gateway(json(query), {})).json()).toEqual({data: {entity: {id: "pg"}}})
		expect(spyResolver).not.toHaveBeenCalled()

		// 4. Reset via ConfigMap restores serving from geo-db
		resolverImpl = async () => ({ok: true, data: {entity: {id: "geo2"}}})
		writeConfig({fields: {entity: "on"}, reset: {entity: "r1"}}, 5_000)
		now += 2_000
		expect(await (await gateway(json(query), {})).json()).toEqual({data: {entity: {id: "geo2"}}})

		// 5. Flip to off: PostGraphile answers, shadow does NOT run
		shadow.mockClear()
		writeConfig({fields: {entity: "off"}}, 8_000)
		now += 2_000
		expect(await (await gateway(json(query), {})).json()).toEqual({data: {entity: {id: "pg"}}})
		await new Promise((r) => setTimeout(r, 10))
		expect(shadow).not.toHaveBeenCalled()
	})

	it("integrates with resolveCard (GEODB_ENTITY_CARDS) across shadow, on, auto-revert, reset, and off", async () => {
		let now = 1_000
		const warn = vi.fn()
		writeConfig({fields: {[ENTITY_CARDS_FIELD]: "shadow"}})
		const sw = createReadSourceSwitch({
			path: filePath,
			defaults: new Map([[ENTITY_CARDS_FIELD, "shadow"]]),
			refreshMs: 1_000,
			now: () => now,
			log: {info: vi.fn(), warn},
			autoRevert: {windowMs: 60_000, minSamples: 3, maxErrorRate: 0.25, maxLatencyMs: 250},
		})

		let failGeo = false
		const readCard = vi.fn(async () => {
			if (failGeo) throw new GeoDbError("timeout", "slow")
			return []
		})
		const deps: CardDeps = {
			mode: "shadow",
			geoDb: {readCard} as unknown as CardDeps["geoDb"],
			geoSpaceOf: async () => "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
			readPostgres: async () => ({values: [], relations: []}),
			log: {info: vi.fn(), warn: vi.fn()},
			readSourceSwitch: sw,
		}

		// shadow: answers from postgres with shadow promise
		const r1 = await resolveCard(deps, "11111111111111111111111111111111", "22222222222222222222222222222222")
		expect(r1.card.source).toBe("postgres")
		expect(r1.shadow).toBeDefined()
		await r1.shadow

		// flip to on: answers from geodb
		writeConfig({fields: {[ENTITY_CARDS_FIELD]: "on"}}, 2_000)
		now += 2_000
		const r2 = await resolveCard(deps, "11111111111111111111111111111111", "22222222222222222222222222222222")
		expect(r2.card.source).toBe("geodb")

		// geo-db errors trigger auto-revert -> falls back to postgres and stops calling geo-db
		failGeo = true
		for (let i = 0; i < 3; i++) {
			const r = await resolveCard(deps, "11111111111111111111111111111111", "22222222222222222222222222222222")
			expect(r.card.source).toBe("postgres")
		}
		expect(sw.isReverted(ENTITY_CARDS_FIELD)).toBe(true)
		readCard.mockClear()
		const r3 = await resolveCard(deps, "11111111111111111111111111111111", "22222222222222222222222222222222")
		expect(r3.card.source).toBe("postgres")
		expect(r3.shadow).toBeUndefined()
		expect(readCard).not.toHaveBeenCalled()

		// reset restores geo-db serving
		failGeo = false
		sw.reset(ENTITY_CARDS_FIELD)
		const r4 = await resolveCard(deps, "11111111111111111111111111111111", "22222222222222222222222222222222")
		expect(r4.card.source).toBe("geodb")
	})

	it("matches GatewayServeFallbackHigh and GatewayServeLatencyHigh defaults and failure classes (#1682)", () => {
		expect(DEFAULT_AUTO_REVERT_CONFIG).toEqual({
			windowMs: 15 * 60_000,
			minSamples: 20,
			maxErrorRate: 0.1,
			maxLatencyMs: 250,
		})
		for (const reason of ["timeout", "overloaded", "client_failed", "bad_request", "error", "id_map"]) {
			expect(isGeoDbErrorReason(reason), reason).toBe(true)
		}
		for (const reason of ["selection", "args", "filter", "not_enabled", "legacy_id", "unmapped_space"]) {
			expect(isGeoDbErrorReason(reason), reason).toBe(false)
		}
	})

	it("ships every field as shadow in api/k8s/v2/api.yaml geodb-read-source ConfigMap", () => {
		const yaml = readFileSync(path.resolve(__dirname, "../../../k8s/v2/api.yaml"), "utf8")
		expect(yaml).toContain("name: geodb-read-source")
		expect(yaml).toContain("GEODB_READ_SOURCE_PATH")
		const match = yaml.match(/read-source\.json:\s*\|\n([\s\S]*?)(?:\n---|\n\S|$)/)
		expect(match).not.toBeNull()
		const body = (match?.[1] ?? "")
			.split("\n")
			.map((line) => line.replace(/^ {4}/, ""))
			.join("\n")
		const parsed = JSON.parse(body) as {fields: Record<string, string>}
		expect(Object.keys(parsed.fields).length).toBeGreaterThanOrEqual(12)
		for (const [field, mode] of Object.entries(parsed.fields)) {
			expect(mode, `field ${field} must ship as shadow`).toBe("shadow")
		}
	})
})
