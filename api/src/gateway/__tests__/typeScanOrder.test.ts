import {parse} from "graphql"
import {describe, expect, it, vi} from "vitest"
import {conformingId} from "../../geodb/card"
import type {EntityShadowDeps} from "../entityShadow"
import {analyzeLightEntities, type LightEntitiesPlan} from "../lightShadow"
import {checkTypeScan} from "../typeScanShadow"

// Postgres pages an unfiltered `entities` by the id it holds (a uuid, compared bytewise); geo-db scans by the conforming id
// of that id. The two are the same string for a v4 id and unrelated for any other legacy id, so a page holding one can sort
// differently in the two stores with no row missing.
const SPACE = "0123456789ab4bcd8ef0123456789abc"
const TYPE = "e550fe517e904b2c8fffdf13408f5634"
const V4_LOW = "10000000000040008000000000000000"
const V4_HIGH = "30000000000040008000000000000000"

/** A legacy (non-v4) id between the two v4 ids in Postgres order whose conforming id sorts outside them. */
function legacyOutOfPlace(): string {
	for (let n = 0; n < 4096; n++) {
		const id = `20000000000060008000000000${n.toString(16).padStart(6, "0")}`
		const conformed = conformingId("entity", id)
		if (conformed !== id && (conformed < V4_LOW || conformed > V4_HIGH)) return id
	}
	throw new Error("no candidate")
}

const query = (first: number) =>
	`query { entities(spaceId: "${SPACE}", typeId: "${TYPE}", first: ${first}) { id name spaceIds } }`

function plan(first: number): LightEntitiesPlan {
	const analysis = analyzeLightEntities(parse(query(first)), null, {})
	if (!analysis.ok) throw new Error(`not planned: ${analysis.reason}`)
	return analysis.plan
}

const deps = (scanned: string[]) =>
	({
		typeScan: true,
		multiSpace: true,
		geoDb: {readTypeScan: vi.fn(async () => scanned)},
		geoSpaceOf: async (space: string) => space,
		log: {info: vi.fn(), warn: vi.fn()},
	}) as unknown as EntityShadowDeps

describe("type scan sequence over a page holding a legacy id", () => {
	const legacy = legacyOutOfPlace()
	// Postgres' page, in its order, and geo-db's whole scan, in conforming-id order.
	const page = [V4_LOW, legacy, V4_HIGH]
	const scanned = page.map((id) => conformingId("entity", id)).sort()

	it("precondition: the two orders really differ", () => {
		expect(page.map((id) => conformingId("entity", id))).not.toEqual(scanned)
	})

	it("matches a complete page whose members agree, whatever the legacy id's place", async () => {
		const check = await checkTypeScan(deps(scanned), plan(800), page, "t")
		expect(check).toMatchObject({result: "match", uncompared: ["legacy_id_order"]})
	})

	it("still flags a complete page with a member geo-db lacks", async () => {
		const missing = scanned.slice(0, 2)
		const check = await checkTypeScan(deps(missing), plan(800), page, "t")
		expect(check.result).toBe("mismatch")
	})

	it("still compares the order of a page of v4 ids only", async () => {
		const ids = [V4_LOW, V4_HIGH]
		const check = await checkTypeScan(deps([...ids].reverse()), plan(800), ids, "t")
		expect(check.result).toBe("mismatch")
	})

	it("leaves a full page with a legacy id uncompared: its cut and order are Postgres' own", async () => {
		const check = await checkTypeScan(deps(scanned), plan(3), page, "t")
		expect(check).toEqual({result: "unsupported", reason: "type_scan_legacy_ids"})
	})
})
