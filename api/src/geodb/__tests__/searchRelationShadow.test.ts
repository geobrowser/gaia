import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {readGatewayConfig} from "../../gateway/config"
import {createGateway, createShadowTracker} from "../../gateway/gateway"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../../gateway/metrics"
import {analyzePointRead} from "../../gateway/pointReadShadows"
import {RELATION_QUERY} from "../../gateway/probe"
import {analyzeSearchOperation} from "../../gateway/searchShadow"
import {
	createSearchRelationShadow,
	readSearchRelationShadowConfig,
	SEARCH_CONNECTION_QUERY,
	SEARCH_QUERY,
	sampleSearchQueries,
} from "../searchRelationShadow"

type Database = NodePgDatabase<Record<string, unknown>>

const ID = "11111111-1111-4111-8111-111111111111"
const log = {warn: vi.fn()}
const names = (list: string[]) => ({execute: vi.fn(async () => ({rows: list.map((text) => ({text}))}))}) as never

beforeEach(() => {
	__resetGatewayMetricsForTests()
	log.warn.mockClear()
})

describe("readSearchRelationShadowConfig", () => {
	it("defaults to on, every 5 minutes, 3 samples", () => {
		expect(readSearchRelationShadowConfig({})).toEqual({enabled: true, intervalMs: 300_000, sample: 3})
	})
	it("turns off only for an explicit off value", () => {
		for (const off of ["0", "off", "false", " OFF "])
			expect(readSearchRelationShadowConfig({GEODB_SEARCH_RELATION_SHADOW: off}).enabled).toBe(false)
		expect(readSearchRelationShadowConfig({GEODB_SEARCH_RELATION_SHADOW: "1"}).enabled).toBe(true)
	})
	it("takes valid tunables, ignores bad ones and caps the sample", () => {
		const c = readSearchRelationShadowConfig({
			GEODB_SEARCH_RELATION_SHADOW_INTERVAL_MS: "1000",
			GEODB_SEARCH_RELATION_SHADOW_SAMPLE: "999",
		})
		expect(c).toEqual({enabled: true, intervalMs: 1000, sample: 10})
		const bad = readSearchRelationShadowConfig({
			GEODB_SEARCH_RELATION_SHADOW_INTERVAL_MS: "x",
			GEODB_SEARCH_RELATION_SHADOW_SAMPLE: "0",
		})
		expect(bad).toEqual({enabled: true, intervalMs: 300_000, sample: 3})
	})
})

describe("operations", () => {
	it("are shapes the existing shadows can settle", () => {
		expect(analyzeSearchOperation("search", parse(SEARCH_QUERY), null, {q: "paris"}).ok).toBe(true)
		expect(analyzeSearchOperation("searchConnection", parse(SEARCH_CONNECTION_QUERY), null, {q: "paris"}).ok).toBe(
			true,
		)
		expect(analyzePointRead("relation", parse(RELATION_QUERY), null, {id: ID}).ok).toBe(true)
	})
})

describe("sampleSearchQueries", () => {
	it("keeps usable, distinct names (2 to 250 chars, not a flag) up to the limit", async () => {
		const db = names(["Paris", "paris ", "-x", "a", "Berlin", "x".repeat(251), "Rome", "Oslo"])
		expect(await sampleSearchQueries(db, "p", 3)).toEqual(["Paris", "Berlin", "Rome"])
	})
	it("wraps to the start when the pivot is near the end", async () => {
		const calls: string[][] = [["Rome"], ["Oslo", "Rome"]]
		const db = {execute: vi.fn(async () => ({rows: (calls.shift() ?? []).map((text) => ({text}))}))} as never
		expect(await sampleSearchQueries(db, "p", 2)).toEqual(["Rome", "Oslo"])
	})
})

describe("createSearchRelationShadow pass", () => {
	const make = (over: {send?: (r: Request) => Promise<Response>; db?: Database; ids?: string[]; sample?: number}) => {
		const send = over.send ?? vi.fn(async () => new Response("{}"))
		const sampleQueries = vi.fn(async () => ["Paris", "Berlin"])
		const sampleIds = vi.fn(async () => over.ids ?? [ID])
		const shadow = createSearchRelationShadow({
			config: {enabled: true, intervalMs: 1000, sample: over.sample ?? 3},
			db: (over.db ?? ({} as Database)) as Database,
			send,
			log,
			sampleQueries,
			sampleIds,
		})
		return {shadow, send: send as ReturnType<typeof vi.fn>, sampleQueries, sampleIds}
	}
	const bodies = async (send: ReturnType<typeof vi.fn>) =>
		Promise.all(
			send.mock.calls.map(async ([r]) => (await (r as Request).json()) as {query: string; variables: object}),
		)

	it("sends each sampled query as search and searchConnection and each id as relation", async () => {
		const {shadow, send} = make({})
		await shadow.runOnce()
		const sent = await bodies(send)
		expect(sent.map((b) => b.query.match(/\{ (\w+)/)?.[1])).toEqual([
			"search",
			"searchConnection",
			"search",
			"searchConnection",
			"relation",
		])
		expect(sent[0]?.variables).toEqual({q: "Paris"})
		expect(sent[4]?.variables).toEqual({id: ID})
	})
	it("asks for at most `sample` queries and ids", async () => {
		const {shadow, sampleQueries, sampleIds} = make({sample: 2})
		await shadow.runOnce()
		expect(sampleQueries).toHaveBeenCalledWith(expect.anything(), expect.any(String), 2)
		expect(sampleIds).toHaveBeenCalledWith(expect.anything(), expect.any(String), 2)
	})
	it("a Postgres sampling failure sends nothing, counts nothing and never throws", async () => {
		const send = vi.fn(async () => new Response("{}"))
		const shadow = createSearchRelationShadow({
			config: {enabled: true, intervalMs: 1000, sample: 3},
			db: {} as Database,
			send,
			log,
			sampleQueries: async () => Promise.reject(new Error("pg down")),
			sampleIds: async () => Promise.reject(new Error("pg down")),
		})
		await shadow.runOnce()
		expect(send).not.toHaveBeenCalled()
		expect(renderGatewayMetrics()).not.toMatch(/gaia_api_gateway_shadow_total\{[^}]*\} [1-9]/)
		expect(log.warn).toHaveBeenCalled()
	})
	it("a failing send is logged and the pass carries on", async () => {
		const send = vi.fn(async () => Promise.reject(new Error("boom")))
		const {shadow} = make({send})
		await shadow.runOnce()
		expect(send).toHaveBeenCalledTimes(5)
		expect(log.warn).toHaveBeenCalled()
	})
	it("start() starts nothing when disabled", () => {
		const s = createSearchRelationShadow({
			config: {enabled: false, intervalMs: 1, sample: 1},
			db: {} as Database,
			send: async () => new Response("{}"),
			log,
		})
		expect(typeof s.start()).toBe("function")
	})
	it("a shadow sees every operation at sample rate 1 through the gateway", async () => {
		const seen: string[] = []
		const mk = () =>
			vi.fn(
				async ({classification}: {classification: {rootFields: string[]}}) =>
					void seen.push(...classification.rootFields),
			)
		const config = {
			...readGatewayConfig({
				GRAPHQL_GATEWAY: "on",
				GRAPHQL_GATEWAY_FIELDS: "search=shadow,searchConnection=shadow,relation=shadow",
			}),
			shadowSampleRate: 1,
		}
		const gateway = createGateway({
			config,
			delegate: async () =>
				new Response(JSON.stringify({data: {}}), {headers: {"content-type": "application/json"}}),
			shadows: new Map([
				["search", mk()],
				["searchConnection", mk()],
				["relation", mk()],
			]),
			random: () => 0,
		})
		const {shadow} = make({send: (r) => Promise.resolve(gateway(r, {} as never))})
		await shadow.runOnce()
		await new Promise((r) => setTimeout(r, 20))
		expect(seen.sort()).toEqual(["relation", "search", "search", "searchConnection", "searchConnection"])
	})
	it("settles each shadow before the next sample, so the geo-db client sees one search at a time", async () => {
		let running = 0
		let peak = 0
		let completed = 0
		const slow = Object.assign(async () => {
			running++
			peak = Math.max(peak, running)
			await new Promise((r) => setTimeout(r, 5))
			running--
			completed++
		}, {})
		const tracker = createShadowTracker()
		const config = {
			...readGatewayConfig({
				GRAPHQL_GATEWAY: "on",
				GRAPHQL_GATEWAY_FIELDS: "search=shadow,searchConnection=shadow,relation=shadow",
			}),
			shadowSampleRate: 1,
		}
		const gateway = createGateway({
			config,
			delegate: async () =>
				new Response(JSON.stringify({data: {}}), {headers: {"content-type": "application/json"}}),
			shadows: new Map([
				["search", slow],
				["searchConnection", slow],
				["relation", slow],
			]),
			random: () => 0,
			trackShadow: tracker.track,
		})
		const send = async (r: Request) => {
			const response = await gateway(r, {} as never)
			await tracker.settled()
			return response
		}
		const {shadow} = make({send})
		await shadow.runOnce()
		expect(completed).toBe(5)
		expect(peak).toBe(1)
	})
})
