import {Hono} from "hono"
import {describe, expect, it, vi} from "vitest"
import {GeoDbError} from "../client"
import type {GeoDbCardConfig} from "../config"
import {createGeoDbRouter} from "../router"
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

function makeDeps(over: Partial<CardDeps> & {readCard?: () => Promise<{prop: string; raw: string}[]>} = {}) {
	const log = {info: vi.fn(), warn: vi.fn()}
	const readCard = over.readCard ?? vi.fn(async () => [{prop: PROP, raw: "Noam"}])
	const deps: CardDeps = {
		mode: "on",
		geoDb: {readCard} as CardDeps["geoDb"],
		geoSpaceOf: async (space) => (space === SPACE ? GEO_SPACE : null),
		readPostgres: vi.fn(async () => pgRows),
		log,
		...over,
	}
	return {deps, log, readCard}
}

describe("resolveCard mode on", () => {
	it("answers from geo-db and does not touch Postgres", async () => {
		const {deps, readCard} = makeDeps()
		const {card} = await resolveCard(deps, ENTITY, SPACE)
		expect(card).toMatchObject({source: "geodb", statements: [{property: PROP, value: "Noam"}]})
		expect(deps.readPostgres).not.toHaveBeenCalled()
		expect(readCard).toHaveBeenCalledWith({geoSpaceId: GEO_SPACE, entityId: ENTITY})
	})

	it.each(["timeout", "client_failed"] as const)("falls back to Postgres on a geo-db %s", async (reason) => {
		const {deps, log} = makeDeps({
			readCard: async () => {
				throw new GeoDbError(reason, "boom")
			},
		})
		const {card} = await resolveCard(deps, ENTITY, SPACE)
		expect(card).toMatchObject({
			source: "postgres",
			fallback: reason,
			statements: [{property: PROP, value: "Noam"}],
		})
		expect(log.warn).toHaveBeenCalledWith(
			"geodb card read failed, falling back to postgres",
			expect.objectContaining({reason}),
		)
	})

	it("falls back when the space is not loaded in geo-db", async () => {
		const {deps} = makeDeps()
		const {card} = await resolveCard(deps, ENTITY, "33333333333343338333333333333333")
		expect(card).toMatchObject({source: "postgres", fallback: "unmapped_space"})
	})

	it("falls back on an unexpected error type", async () => {
		const {deps} = makeDeps({
			readCard: async () => {
				throw new Error("socket closed")
			},
		})
		const {card} = await resolveCard(deps, ENTITY, SPACE)
		expect(card).toMatchObject({source: "postgres", fallback: "error"})
	})
})

describe("resolveCard mode shadow", () => {
	it("answers from Postgres and logs nothing when geo-db agrees", async () => {
		const {deps, log} = makeDeps({mode: "shadow"})
		const {card, shadow} = await resolveCard(deps, ENTITY, SPACE)
		await shadow
		expect(card.source).toBe("postgres")
		expect(log.warn).not.toHaveBeenCalled()
	})

	it("logs a structured diff when the stores differ, and still answers from Postgres", async () => {
		const {deps, log} = makeDeps({mode: "shadow", readCard: async () => [{prop: PROP, raw: "Noam S."}]})
		const {card, shadow} = await resolveCard(deps, ENTITY, SPACE)
		await shadow
		expect(card.statements).toEqual([{property: PROP, value: "Noam"}])
		expect(log.warn).toHaveBeenCalledWith(
			"geodb card diff",
			expect.objectContaining({
				entityId: ENTITY,
				spaceId: SPACE,
				onlyPostgres: 1,
				onlyGeoDb: 1,
				samplePostgres: [{prop: PROP, kind: "text", norm: "Noam"}],
				sampleGeoDb: [{prop: PROP, raw: "Noam S."}],
			}),
		)
	})

	it("never fails the response when geo-db errors", async () => {
		const {deps, log} = makeDeps({
			mode: "shadow",
			readCard: async () => {
				throw new GeoDbError("timeout", "slow")
			},
		})
		const {card, shadow} = await resolveCard(deps, ENTITY, SPACE)
		await expect(shadow).resolves.toBeUndefined()
		expect(card.source).toBe("postgres")
		expect(log.warn).toHaveBeenCalledWith(
			"geodb card shadow read failed",
			expect.objectContaining({reason: "timeout"}),
		)
	})
})

describe("GET /geodb/entities/:entityId/card", () => {
	const config: GeoDbCardConfig = {
		mode: "on",
		clientBin: "geodb-client",
		keyPath: "",
		bindingPath: "",
		spaceMapPath: "",
		timeoutMs: 500,
		poolSize: 4,
	}

	function setup() {
		const db = {execute: vi.fn(async () => ({rows: []}))}
		const readCard = vi.fn(async () => [{prop: PROP, raw: "Noam"}])
		// biome-ignore lint/suspicious/noExplicitAny: test mock
		const router = createGeoDbRouter(db as any, config, {
			geoDb: {readCard},
			geoSpaceOf: async () => GEO_SPACE,
			log: {info: vi.fn(), warn: vi.fn()},
		})
		const app = new Hono()
		app.route("/geodb", router)
		return {app, db, readCard}
	}

	it("returns the card from geo-db", async () => {
		const {app, db} = setup()
		const res = await app.request(`/geodb/entities/${ENTITY}/card?spaceId=${SPACE}`)
		expect(res.status).toBe(200)
		expect(await res.json()).toMatchObject({source: "geodb", entityId: ENTITY, spaceId: SPACE})
		expect(db.execute).not.toHaveBeenCalled()
	})

	it("rejects ids that are not UUIDs", async () => {
		const {app} = setup()
		expect((await app.request(`/geodb/entities/nope/card?spaceId=${SPACE}`)).status).toBe(400)
		expect((await app.request(`/geodb/entities/${ENTITY}/card`)).status).toBe(400)
	})
})
