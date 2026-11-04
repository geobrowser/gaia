import {mkdtempSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {cid} from "../ids.js"
import {createMetrics} from "../metrics.js"
import {systemRelationId} from "../proposals.js"
import {
	applySpaceEntity,
	handleSpaceCreationMessage,
	parseSpaceRegistered,
	PendingSpaces,
	spaceRows,
	sweepPending,
	type SpaceEntityDeps,
} from "../spaceEntities.js"
import {Store} from "../state.js"

const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const OWNER = "1111111111111111111111111111111111111111"
const GEO = "daf0bc83ca744e1a9ee0377967846846"
const tmp = () => mkdtempSync(join(tmpdir(), "space-entities-"))
const bytes = (h: string) => Buffer.from(h, "hex")
const field = (n: number, b: Buffer) => Buffer.concat([Buffer.from([(n << 3) | 2, b.length]), b])

/** HermesCreateSpace: space_id = 1, eoa_space = 3 / default_dao_space = 4 (address = 1), meta = 5 { block_number = 3 }. */
function message(kind: "eoa" | "dao", block = 300, withMeta = true): Buffer {
	const parts = [field(1, bytes(SPACE)), field(kind === "eoa" ? 3 : 4, field(1, bytes(OWNER)))]
	if (withMeta) parts.push(field(5, Buffer.from([(3 << 3) | 0, (block & 0x7f) | 0x80, block >> 7])))
	return Buffer.concat(parts)
}

function fixture(founded = true) {
	const store = new Store(tmp())
	if (founded) store.spaces[SPACE] = {geo: GEO} as never
	const applied: string[] = []
	const deps: SpaceEntityDeps = {
		db: {
			apply: async (zon: string) => {
				applied.push(zon)
				return {rc: 0, out: "", err: ""}
			},
			waitReady: async () => [],
			found: async () => GEO,
		},
		sealer: async () => "5".repeat(64),
		store,
		metrics: createMetrics(),
		beat: async () => {},
		sleep: async () => {},
		publish: () => {},
		pending: new PendingSpaces(tmp()),
	}
	return {store, applied, deps}
}
const parse = (kind: "eoa" | "dao") => parseSpaceRegistered(undefined, message(kind)) as NonNullable<ReturnType<typeof parseSpaceRegistered>>

describe("parseSpaceRegistered", () => {
	it("reads space, kind, address and block; ignores SPACE_ADDRESS_UPDATED and a message without meta; rejects malformed or payload-less", () => {
		expect(parse("eoa")).toEqual({space: SPACE, kind: "EoaSpace", address: OWNER, block: 300n})
		expect(parse("dao").kind).toBe("DaoSpace")
		expect(parseSpaceRegistered("SPACE_ADDRESS_UPDATED", message("eoa"))).toBeNull()
		expect(parseSpaceRegistered(undefined, message("eoa", 300, false))).toBeNull()
		expect(() => parseSpaceRegistered(undefined, field(1, bytes(SPACE)))).toThrow()
		expect(() => parseSpaceRegistered(undefined, Buffer.from([0x0a, 0x01, 0x01]))).toThrow()
	})
})

describe("spaceRows", () => {
	it("is the four values and three type relations map_space_registered writes", () => {
		const r = spaceRows(parse("dao"))
		expect(r.values.map((v) => v.text ?? v.integer)).toEqual([
			`0x${OWNER}`,
			"300",
			"Space a19c345a-b986-6679-b001-d7d2138d88a1",
			"System entity for space a19c345a-b986-6679-b001-d7d2138d88a1",
		])
		expect(r.relations.map((x) => x.to_entity_id)).toEqual([
			"2ff7ea09-8b9e-50bc-9be7-8a0cafa268d0",
			"f4ce7263-ed14-56c5-aafe-8b74428ce812",
			"afd76215-db11-5cba-81b9-e7f77f865805",
		])
		expect(r.relations[2]?.id).toBe(systemRelationId("DaoSpace", SPACE))
		expect(spaceRows(parse("eoa")).relations[2]?.to_entity_id).toBe("09d28123-178f-5828-b85a-81979389c746")
	})
})

describe("applySpaceEntity", () => {
	it("creates the entity with its values and relations once; a repeat is a no-op", async () => {
		const f = fixture()
		expect(await applySpaceEntity(f.deps, parse("eoa"), {partition: 0, offset: "1"})).toBe("applied")
		expect(await applySpaceEntity(f.deps, parse("eoa"), {partition: 0, offset: "1"})).toBe("noop")
		expect(f.applied).toHaveLength(1)
		const zon = f.applied[0] as string
		expect(zon).toContain(`.create_entity = "${cid("entity", SPACE)}"`)
		expect((zon.match(/\.add = /g) ?? []).length).toBe(3)
		expect((zon.match(/\.set = /g) ?? []).length).toBe(4)
	})
	it("parks an entity for a Space not founded yet without founding it, and applies it once the Space exists", async () => {
		const f = fixture(false)
		expect(await applySpaceEntity(f.deps, parse("eoa"), {partition: 0, offset: "1"})).toBe("pending")
		expect(f.applied).toHaveLength(0)
		expect(f.deps.pending.size).toBe(1)
		expect(new PendingSpaces(f.deps.pending.dir).list()).toEqual([parse("eoa")]) // survives a restart
		expect(await sweepPending(f.deps)).toBe(0)
		f.store.spaces[SPACE] = {geo: GEO} as never
		expect(await sweepPending(f.deps)).toBe(1)
		expect(f.applied).toHaveLength(1)
		expect(f.deps.pending.size).toBe(0)
	})
	it("a refused intent is dead-lettered and not journaled", async () => {
		const f = fixture()
		f.deps.db.apply = async () => ({rc: 1, out: "", err: "InvalidIntent"})
		expect(await applySpaceEntity(f.deps, parse("eoa"), {partition: 0, offset: "9"})).toBe("dead_letter")
		expect(f.store.hasEntity(SPACE, cid("entity", SPACE))).toBe(false)
	})
})

describe("handleSpaceCreationMessage", () => {
	it("applies a creation, ignores address updates and empty values, dead-letters garbage", async () => {
		const f = fixture()
		expect(await handleSpaceCreationMessage(f.deps, 0, {offset: "1", value: message("eoa")})).toBe("applied")
		expect(await handleSpaceCreationMessage(f.deps, 0, {offset: "2", value: message("eoa"), headers: {"event-type": Buffer.from("SPACE_ADDRESS_UPDATED")}})).toBe("ignored")
		expect(await handleSpaceCreationMessage(f.deps, 0, {offset: "3", value: null})).toBe("ignored")
		expect(await handleSpaceCreationMessage(f.deps, 0, {offset: "4", value: Buffer.from([1])})).toBe("dead_letter")
	})
})
