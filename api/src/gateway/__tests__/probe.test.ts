import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {parse} from "graphql"
import {describe, expect, it, vi} from "vitest"

type Database = NodePgDatabase<Record<string, unknown>>

import {readGatewayConfig} from "../config"
import {createGateway} from "../gateway"
import {analyzeMetas} from "../metasShadow"
import {analyzePointRead} from "../pointReadShadows"
import {createGatewayProbe, METAS_QUERY, RELATION_QUERY, readGatewayProbeConfig, sampleRelationIds} from "../probe"

const ID = "11111111-1111-4111-8111-111111111111"
const ids = (list: string[]) =>
	({execute: vi.fn(async () => ({rows: list.map((id) => ({id}))}))}) as unknown as Database
const log = {warn: vi.fn()}

describe("config", () => {
	it("is off unless the flag says on, and clamps tunables", () => {
		expect(readGatewayProbeConfig({}).enabled).toBe(false)
		expect(readGatewayProbeConfig({GATEWAY_PROBE: "1"}).enabled).toBe(true)
		const c = readGatewayProbeConfig({GATEWAY_PROBE_RELATIONS: "999"})
		expect(c.relations).toBe(20)
		expect(c.intervalMs).toBe(60_000)
	})
})

describe("probe operations", () => {
	it("are shapes the shadows can settle", () => {
		const metas = analyzeMetas(parse(METAS_QUERY), null, {})
		expect(metas.ok).toBe(true)
		const relation = analyzePointRead("relation", parse(RELATION_QUERY), null, {id: ID})
		expect(relation.ok).toBe(true)
	})
})

describe("sampleRelationIds", () => {
	it("wraps to the start when the pivot is near the end, keeping each id once", async () => {
		const calls: string[][] = [["b"], ["a", "b"]]
		const db = {
			execute: vi.fn(async () => ({rows: (calls.shift() ?? []).map((id) => ({id}))})),
		} as unknown as Database
		expect(await sampleRelationIds(db, "p", 3)).toEqual(["b", "a"])
	})
})

describe("probe", () => {
	it("sends metas and each sampled relation, and a shadow sees every one at sample rate 1", async () => {
		const seen: string[] = []
		const shadow = vi.fn(async ({classification}: {classification: {rootFields: string[]}}) => {
			seen.push(...classification.rootFields)
		})
		const config = {
			...readGatewayConfig({GRAPHQL_GATEWAY: "on", GRAPHQL_GATEWAY_FIELDS: "metas=shadow,relation=shadow"}),
			shadowSampleRate: 1,
		}
		const gateway = createGateway({
			config,
			delegate: async () =>
				new Response(JSON.stringify({data: {}}), {headers: {"content-type": "application/json"}}),
			shadows: new Map([
				["metas", shadow],
				["relation", shadow],
			]),
			random: () => 0.5,
		})
		const probe = createGatewayProbe({
			config: {enabled: true, intervalMs: 1, relations: 2},
			db: ids(["a", "b"]),
			send: (r) => gateway(r, {}),
			log,
		})
		await probe.runOnce()
		await new Promise((r) => setTimeout(r, 10))
		expect(seen).toEqual(["metas", "relation", "relation"])
	})

	it("survives a failing gateway and a failing sample", async () => {
		const probe = createGatewayProbe({
			config: {enabled: true, intervalMs: 1, relations: 1},
			db: {execute: async () => Promise.reject(new Error("down"))} as unknown as Database,
			send: async () => Promise.reject(new Error("boom")),
			log,
		})
		await expect(probe.runOnce()).resolves.toBeUndefined()
		expect(log.warn).toHaveBeenCalled()
	})

	it("start is inert when disabled", () => {
		const probe = createGatewayProbe({
			config: {enabled: false, intervalMs: 1, relations: 1},
			db: ids([]),
			send: async () => new Response(""),
			log,
		})
		expect(probe.start()()).toBeUndefined()
	})
})
