import {describe, expect, it, vi} from "vitest"
import {conformingId, type GeoCardRow} from "../../geodb/card"
import {cardsDocument, type GeoDbCardClient} from "../../geodb/client"
import {type EntityShadowDeps, NAME_PROPERTY, readNames, TYPES_PROPERTY} from "../entityShadow"
import {checkNestedEntities} from "../nestedEntity"
import {withoutExcludedTypes} from "../searchShadow"

// The shadow reads that only inspect one property must say which, so geo-db does not read_blob every unrelated detached
// text of every entity across every Space; the nested read names the Spaces PostGraphile listed when it listed them.

const ENTITY = "e1c3ad9f-5a3a-4a1c-8c0b-d1b45b4d6a10"
const HEX = conformingId("entity", ENTITY)
const GEO_SPACE = "22222222222242228222222222222222"
const GEO_SPACE_B = "44444444444442228444444444444444"
const GEO_SPACE_C = "55555555555542228555555555555555"
const GAIA_A = "11111111-1111-4111-8111-111111111111"
const GAIA_B = "33333333-3333-4333-8333-333333333333"

type Args = Parameters<NonNullable<GeoDbCardClient["readCards"]>>[0]

function depsFor(spaces: string[], map: Record<string, string> = {}) {
	const readCards = vi.fn(async (_args: Args): Promise<GeoCardRow[]> => [])
	const deps = {
		geoDb: {readCards},
		allGeoSpaces: async () => spaces,
		geoSpaceOf: async (space: string) => map[space] ?? null,
		everySpaceBudget: {tryAcquire: vi.fn(() => true)},
		log: {info: vi.fn(), warn: vi.fn()},
	} as unknown as EntityShadowDeps
	return {deps, readCards}
}

describe("cardsDocument", () => {
	it("names one property when asked, and none otherwise", () => {
		expect(cardsDocument([HEX], {property: NAME_PROPERTY})).toContain(`.property = "${NAME_PROPERTY}"`)
		expect(cardsDocument([HEX])).not.toContain(".property")
	})
})

describe("withoutExcludedTypes", () => {
	it("reads only the Type property", async () => {
		const readCards = vi.fn(async (_args: Args): Promise<GeoCardRow[]> => [])
		await withoutExcludedTypes(readCards, [GEO_SPACE], [HEX], 5)
		expect(readCards.mock.calls[0]?.[0].property).toBe(TYPES_PROPERTY)
	})
})

describe("readNames", () => {
	it("reads only the Name property", async () => {
		const {deps, readCards} = depsFor([GEO_SPACE])
		await readNames(deps, [HEX], "t")
		expect(readCards.mock.calls[0]?.[0].property).toBe(NAME_PROPERTY)
	})
})

describe("checkNestedEntities scope", () => {
	const items = (...spaceIds: (string[] | undefined)[]) =>
		({
			items: spaceIds.map((s, i) => ({
				id: i === 0 ? ENTITY : `e1c3ad9f-5a3a-4a1c-8c0b-d1b45b4d6a1${i}`,
				spaceIds: s,
				lists: [],
			})),
		}) as unknown as Parameters<typeof checkNestedEntities>[1]

	it("reads only the deduplicated mapped Spaces the items list", async () => {
		const {deps, readCards} = depsFor([GEO_SPACE, GEO_SPACE_B, GEO_SPACE_C], {
			[GAIA_A]: GEO_SPACE,
			[GAIA_B]: GEO_SPACE_B,
		})
		await checkNestedEntities(deps, items([GAIA_A], [GAIA_A, GAIA_B]), "t")
		expect([...(readCards.mock.calls[0]?.[0].geoSpaceIds as string[])].sort()).toEqual([GEO_SPACE, GEO_SPACE_B])
	})

	it("falls back to every Space when an item lists none", async () => {
		const {deps, readCards} = depsFor([GEO_SPACE, GEO_SPACE_B], {[GAIA_A]: GEO_SPACE})
		await checkNestedEntities(deps, items([GAIA_A], undefined), "t")
		expect(readCards.mock.calls[0]?.[0].geoSpaceIds).toEqual([GEO_SPACE, GEO_SPACE_B])
	})

	it("falls back to every Space when a listed Space has no geo-db mapping", async () => {
		const {deps, readCards} = depsFor([GEO_SPACE, GEO_SPACE_B], {[GAIA_A]: GEO_SPACE})
		await checkNestedEntities(deps, items([GAIA_A, GAIA_B]), "t")
		expect(readCards.mock.calls[0]?.[0].geoSpaceIds).toEqual([GEO_SPACE, GEO_SPACE_B])
	})
})
