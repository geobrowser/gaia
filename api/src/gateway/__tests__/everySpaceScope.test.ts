import {describe, expect, it, vi} from "vitest"
import {conformingId} from "../../geodb/card"
import {type EntityShadowDeps, MAX_EVERY_SPACES, readNames, scopeOfSpaces} from "../entityShadow"
import {checkNestedEntities} from "../nestedEntity"
import {checkPropertyInfo, type PropertyInfos} from "../propertyInfo"

const spacesOf = (n: number) => Array.from({length: n}, (_, i) => (i + 1).toString(16).padStart(32, "0"))
const ENTITY = "e1c3ad9f-5a3a-4a1c-8c0b-d1b45b4d6a10"

function depsFor(spaces: string[]) {
	const readCards = vi.fn(
		async (_args: {geoSpaceIds: string[] | "every"; entityIds: string[]; names?: boolean}) => [],
	)
	const deps = {
		geoDb: {readCards},
		allGeoSpaces: async () => spaces,
		everySpaceBudget: {tryAcquire: vi.fn(() => true)},
		log: {info: vi.fn(), warn: vi.fn()},
	} as unknown as EntityShadowDeps
	return {deps, readCards}
}

describe("scopeOfSpaces (geo-db DR-177-583)", () => {
	it("lists the Spaces while they fit one line, and asks for the every_space scope past it", () => {
		const fits = spacesOf(MAX_EVERY_SPACES)
		expect(scopeOfSpaces(fits)).toBe(fits)
		expect(scopeOfSpaces(spacesOf(MAX_EVERY_SPACES + 1))).toBe("every")
	})
})

describe("the batched card reads across every Space, past 1024 Spaces", () => {
	const many = spacesOf(MAX_EVERY_SPACES + 555)

	it("readNames reads the names in the every_space scope instead of giving up", async () => {
		const {deps, readCards} = depsFor(many)
		expect(await readNames(deps, [conformingId("entity", ENTITY)], "t")).not.toBeNull()
		expect(readCards).toHaveBeenCalledTimes(1)
		expect(readCards.mock.calls[0]?.[0].geoSpaceIds).toBe("every")
	})

	it("the nested entity read uses the every_space scope", async () => {
		const {deps, readCards} = depsFor(many)
		const verdict = await checkNestedEntities(
			deps,
			{items: [{id: ENTITY}]} as unknown as Parameters<typeof checkNestedEntities>[1],
			"t",
		)
		expect(readCards).toHaveBeenCalledTimes(1)
		expect(readCards.mock.calls[0]?.[0].geoSpaceIds).toBe("every")
		void verdict
	})

	it("the property info read uses the every_space scope", async () => {
		const {deps, readCards} = depsFor(many)
		const info = {
			items: [
				{
					id: ENTITY,
					dataTypeId: null,
					dataTypeName: null,
					renderableTypeId: null,
					renderableTypeName: null,
					format: null,
					isType: false,
				},
			],
			incomplete: false,
		} as unknown as PropertyInfos
		await checkPropertyInfo(deps, info, "t")
		expect(readCards).toHaveBeenCalledTimes(1)
		expect(readCards.mock.calls[0]?.[0].geoSpaceIds).toBe("every")
	})

	it("still names the Spaces of a store that fits one line", async () => {
		const few = spacesOf(3)
		const {deps, readCards} = depsFor(few)
		await readNames(deps, [conformingId("entity", ENTITY)], "t")
		expect(readCards.mock.calls[0]?.[0].geoSpaceIds).toEqual(few)
	})
})
