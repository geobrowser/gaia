import {ContentIds, SystemIds} from "@geoprotocol/geo-sdk"
import {Effect} from "effect"
import {Hono} from "hono"
import {beforeEach, describe, expect, it, vi} from "vitest"

import {conformingId, type GeoCardRow} from "../../geodb/card"
import {type GeoDbCardClient, GeoDbError} from "../../geodb/client"
import {createProfileRouter} from "../index"
import {
	__resetProfileShadowMetricsForTests,
	createProfileShadow,
	readProfileShadowConfig,
	renderProfileShadowMetrics,
} from "../shadow"
import type {Profile} from "../types"

const NAME = conformingId("entity", SystemIds.NAME_PROPERTY)
const AVATAR = conformingId("entity", ContentIds.AVATAR_PROPERTY)
const IMAGE_URL = conformingId("entity", SystemIds.IMAGE_URL_PROPERTY)
const SPACE = "22222222222222222222222222222222"
const ENTITY = "11111111111141118111111111111111"
const IMAGE = "33333333333343338333333333333333"

const served = (over: Partial<Profile> = {}): Profile => ({
	entityId: ENTITY,
	spaceId: SPACE,
	name: "Ada",
	avatarUrl: "https://img/a.png",
	address: "0xab28066d9a7ddFF52B67dF699592BA7060e0d3b9",
	...over,
})

const count = (surface: string, result: string) => {
	const m = renderProfileShadowMetrics().match(
		new RegExp(`gaia_api_profile_shadow_total\\{surface="${surface}",result="${result}"\\} (\\d+)`),
	)
	return Number(m?.[1])
}

function setup(cards: Record<string, GeoCardRow[] | Error>, spaceMap: Record<string, string | null> = {[SPACE]: "s"}) {
	const readCard = vi.fn(async ({entityId}: {entityId: string}) => {
		const c = cards[entityId]
		if (c instanceof Error) throw c
		return c ?? []
	})
	const geoDb = {readCard} as unknown as GeoDbCardClient
	const warn = vi.fn()
	const env = {GEODB_PROFILE_SHADOW: "on", ...{}}
	const shadow = createProfileShadow({
		config: readProfileShadowConfig(env),
		geoDb,
		geoSpaceOf: async (id) => spaceMap[id] ?? null,
		log: {warn},
	})
	return {shadow, readCard, warn}
}

const settle = () => new Promise((r) => setTimeout(r, 0))

const good: Record<string, GeoCardRow[]> = {
	[ENTITY]: [
		{prop: NAME, raw: "Ada"},
		{prop: AVATAR, raw: IMAGE},
	],
	[IMAGE]: [{prop: IMAGE_URL, raw: "https://img/a.png"}],
}

describe("profile shadow", () => {
	beforeEach(() => __resetProfileShadowMetricsForTests())

	it("is off by default and exports nothing until mounted", () => {
		expect(readProfileShadowConfig({}).enabled).toBe(false)
		expect(renderProfileShadowMetrics()).toBe("")
	})

	it("counts a match when name and avatar url agree", async () => {
		const {shadow} = setup(good)
		shadow.profiles("batch", [served()])
		await settle()
		expect(count("batch", "match")).toBe(1)
		expect(count("batch", "failed")).toBe(0)
	})

	it("counts a mismatch on a different name or avatar, and logs counts only", async () => {
		const {shadow, warn} = setup(good)
		shadow.profiles("space", [served({name: "Grace"})])
		shadow.profiles("address", [served({avatarUrl: "https://img/other.png"})])
		await settle()
		expect(count("space", "mismatch")).toBe(1)
		expect(count("address", "mismatch")).toBe(1)
		for (const call of warn.mock.calls) expect(JSON.stringify(call)).not.toMatch(/Ada|Grace|img|1111|2222/)
	})

	it("treats no name and no avatar on both sides as a match, and an empty served name as none", async () => {
		const {shadow} = setup({[ENTITY]: []})
		shadow.profiles("batch", [served({name: null, avatarUrl: null}), served({name: "", avatarUrl: null})])
		await settle()
		expect(count("batch", "match")).toBe(2)
	})

	it("counts a mismatch when geo-db holds nothing Postgres served", async () => {
		const {shadow} = setup({[ENTITY]: []})
		shadow.profiles("batch", [served()])
		await settle()
		expect(count("batch", "mismatch")).toBe(1)
	})

	it("is unsupported with no front page entity, an unloaded Space, or two avatars", async () => {
		const {shadow} = setup({
			[ENTITY]: [
				{prop: AVATAR, raw: IMAGE},
				{prop: AVATAR, raw: "44444444444444444444444444444444"},
			],
		})
		shadow.profiles("batch", [served({entityId: null})])
		shadow.profiles("batch", [served({spaceId: "99999999999999999999999999999999"})])
		shadow.profiles("batch", [served()])
		await settle()
		expect(count("batch", "unsupported")).toBe(3)
		expect(renderProfileShadowMetrics()).toContain('reason="no_front_page"} 1')
		expect(renderProfileShadowMetrics()).toContain('reason="unmapped_space"} 1')
		expect(renderProfileShadowMetrics()).toContain('reason="ambiguous"} 1')
	})

	it("counts a failed geo-db read and never throws", async () => {
		const {shadow} = setup({[ENTITY]: new GeoDbError("timeout", "slow")})
		shadow.profiles("address", [served()])
		await settle()
		expect(count("address", "failed")).toBe(1)
	})

	it("skips default profiles (no Space) and compares only the first few", async () => {
		const {shadow, readCard} = setup(good)
		shadow.profiles("batch", [{entityId: null, spaceId: SPACE, name: null, avatarUrl: null, address: SPACE}])
		await settle()
		expect(readCard).not.toHaveBeenCalled()
		shadow.profiles(
			"batch",
			Array.from({length: 9}, () => served()),
		)
		await settle()
		expect(count("batch", "match")).toBe(5)
	})

	it("drops a comparison past maxInFlight as unsupported busy", async () => {
		const {shadow} = setup(good)
		shadow.profiles(
			"batch",
			Array.from({length: 5}, () => served()),
		)
		const busy = createProfileShadow({
			config: {enabled: true, maxPerResponse: 5, maxInFlight: 1},
			geoDb: {readCard: async () => []} as unknown as GeoDbCardClient,
			geoSpaceOf: async () => "s",
			log: {warn: vi.fn()},
		})
		busy.profiles(
			"space",
			Array.from({length: 3}, () => served()),
		)
		await settle()
		expect(renderProfileShadowMetrics()).toContain('surface="space",reason="busy"} 2')
	})

	it("does nothing when the flag is off", async () => {
		const readCard = vi.fn()
		const off = createProfileShadow({
			config: readProfileShadowConfig({}),
			geoDb: {readCard} as unknown as GeoDbCardClient,
			geoSpaceOf: async () => "s",
			log: {warn: vi.fn()},
		})
		off.profiles("batch", [served()])
		await settle()
		expect(readCard).not.toHaveBeenCalled()
	})
})

describe("profile router wiring", () => {
	const row = {
		entity_id: "11111111-1111-4111-8111-111111111111",
		space_id: "22222222-2222-2222-2222-222222222222",
		space_address: "0xab28066d9a7ddFF52B67dF699592BA7060e0d3b9",
		entity_name: "Ada",
		avatar_url: null,
	}
	const build = () => {
		const calls: [string, Profile[]][] = []
		const db = {execute: vi.fn(async () => ({rows: [row]}))}
		const runtime = {runPromise: <A, E>(e: Effect.Effect<A, E, never>) => Effect.runPromise(e)}
		// biome-ignore lint/suspicious/noExplicitAny: test mock
		const router = createProfileRouter(db as any, runtime as any, {profiles: (s, p) => calls.push([s, [...p]])})
		const app = new Hono()
		app.route("/profile", router)
		return {app, calls}
	}

	it("hands each surface's served profiles to the shadow and leaves the response unchanged", async () => {
		const {app, calls} = build()
		const a = await app.request(`/profile/address/${row.space_address.toLowerCase()}`)
		const s = await app.request(`/profile/space/${row.space_id}`)
		const b = await app.request("/profile/batch", {
			method: "POST",
			headers: {"content-type": "application/json"},
			body: JSON.stringify({spaceIds: [row.space_id]}),
		})
		expect([a.status, s.status, b.status]).toEqual([200, 200, 200])
		expect(((await s.json()) as Profile).name).toBe("Ada")
		expect(calls.map(([surface, p]) => [surface, p.length])).toEqual([
			["address", 1],
			["space", 1],
			["batch", 1],
		])
	})
})
