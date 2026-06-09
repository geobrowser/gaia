/**
 * Integration tests for the v2 entity-diff endpoint (/v2/versioned/entities/:id/diff).
 *
 * Covers the v2-only enrichments layered over the shared v1 diff:
 *   - name resolution (propertyName / typeName / toEntityName)
 *   - media-URL inlining, versioned before/after (cover swap: before→old, after→new) + video
 *   - snapshot mode (no fromEditId → all-added diff)
 *   - rich block shape (blockName + block.values[] + block.relations[] with spaceId)
 *
 * Runs against a real PostgreSQL DB. Prerequisites:
 *   - DATABASE_URL set, migrations applied (bun run db:migrate)
 * Skipped automatically when DATABASE_URL is unset (mirrors the other integration suites).
 */

import {drizzle} from "drizzle-orm/node-postgres"
import {Hono} from "hono"
import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {runtime} from "../../services/runtime"
import {normalizeUuid} from "../../utils/uuid"
import {createVersionedV2Router} from "../v2"

const DATABASE_URL = process.env.DATABASE_URL
const SKIP = !DATABASE_URL

// grc-20 system ids
const NAME_PROPERTY = "a126ca53-0c8e-48d5-b888-82c734c38935"
const TYPES = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"
const BLOCKS = "beaba5cb-a677-41a8-b353-77030613fc70"
const IMAGE_TYPE = "ba4e4146-0010-499d-a0a3-caaa7f579d0e"
const VIDEO_TYPE = "d7a4817c-9795-405b-93e2-12df759c43f8"
const IMAGE_URL = "8a743832-c094-4a62-b665-0c3cc2f9c7bc"
const DATA_BLOCK = "b8803a86-65de-412b-bb35-7e0c84adf473"
const VIEW_PROPERTY = "1907fd1c-8111-4a3c-a378-b1f353425b65"

// Fixture ids — prefix 90000000-* for isolation + cleanup
const SPACE = "90000000-0001-4000-8000-000000000001"
const EDIT1 = "90000000-000a-4000-8000-000000000001"
const EDIT2 = "90000000-000a-4000-8000-000000000002"
const V1 = 9100
const V2 = 9200
const PAGE = "90000000-0002-4000-8000-000000000001"
const OLD_IMG = "90000000-0003-4000-8000-000000000001"
const NEW_IMG = "90000000-0003-4000-8000-000000000002"
const VID = "90000000-0003-4000-8000-000000000003"
const TOPIC = "90000000-0004-4000-8000-000000000001"
const BLOCK_DATA = "90000000-0005-4000-8000-000000000001"
const TARGET1 = "90000000-0006-4000-8000-000000000001"
const TARGET2 = "90000000-0006-4000-8000-000000000002"
const FILTER_PROP = "90000000-0007-4000-8000-000000000001"
const T_COVER = "90000000-0008-4000-8000-000000000001"
const T_TRAILER = "90000000-0008-4000-8000-000000000002"
const T_TOPIC = "90000000-0008-4000-8000-000000000003"
const T_COLLECTION_ITEM = "90000000-0008-4000-8000-000000000004"
const R_COVER = "90000000-0009-4000-8000-000000000001" // stable relationId → swap
const CONFIG_ENTITY = "90000000-000b-4000-8000-000000000001" // reified BLOCKS relation entity (holds config)
const VIEW_TARGET = "90000000-000c-4000-8000-000000000001"

const NAMES: Record<string, string> = {
	[NAME_PROPERTY]: "Name",
	[FILTER_PROP]: "Filter",
	[T_COVER]: "Cover",
	[T_TRAILER]: "Trailer",
	[T_TOPIC]: "Related topic",
	[T_COLLECTION_ITEM]: "Collection item",
	[TOPIC]: "Identity Standards",
	[TARGET1]: "Target One",
	[TARGET2]: "Target Two",
	[PAGE]: "Demo Page",
	[VIEW_TARGET]: "Gallery View",
}
const ALL_IDS = [
	SPACE,
	EDIT1,
	EDIT2,
	PAGE,
	OLD_IMG,
	NEW_IMG,
	VID,
	TOPIC,
	BLOCK_DATA,
	TARGET1,
	TARGET2,
	FILTER_PROP,
	T_COVER,
	T_TRAILER,
	T_TOPIC,
	T_COLLECTION_ITEM,
	CONFIG_ENTITY,
	VIEW_TARGET,
]

const base = `/v2/versioned/entities/${PAGE}/diff?spaceId=${SPACE}`

async function seed(pool: Pool) {
	const c = await pool.connect()
	try {
		await c.query("BEGIN")
		await c.query(
			`INSERT INTO spaces (id, type, address) VALUES ($1,'DAO','0x0000000000000000000000000000000000000099') ON CONFLICT (id) DO NOTHING`,
			[SPACE],
		)
		for (const id of [
			PAGE,
			OLD_IMG,
			NEW_IMG,
			VID,
			TOPIC,
			BLOCK_DATA,
			TARGET1,
			TARGET2,
			FILTER_PROP,
			T_COVER,
			T_TRAILER,
			T_TOPIC,
			T_COLLECTION_ITEM,
			CONFIG_ENTITY,
			VIEW_TARGET,
		]) {
			await c.query(
				`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) VALUES ($1,'2026-05-25T00:00:00Z','1',$2,'1') ON CONFLICT DO NOTHING`,
				[id, "2026-05-25T00:00:00Z"],
			)
		}
		await c.query(
			`INSERT INTO edit_versions (edit_id, block_number, sequence, version_key, created_at, name) VALUES ($1,1,0,$2,'2026-05-25T00:00:00Z','Before') ON CONFLICT DO NOTHING`,
			[EDIT1, V1],
		)
		await c.query(
			`INSERT INTO edit_versions (edit_id, block_number, sequence, version_key, created_at, name) VALUES ($1,2,0,$2,'2026-05-25T00:00:01Z','After') ON CONFLICT DO NOTHING`,
			[EDIT2, V2],
		)

		// live NAME values → name resolution
		for (const [id, name] of Object.entries(NAMES)) {
			await c.query(
				`INSERT INTO "values" (id, property_id, entity_id, space_id, text) VALUES ($1,$2,$3,$4,$5) ON CONFLICT (id) DO UPDATE SET text=EXCLUDED.text`,
				[`v2t-nm-${id}`, NAME_PROPERTY, id, SPACE, name],
			)
		}

		const valVer = (entity: string, prop: string, vfrom: number, vto: number | null, text: string) =>
			c.query(
				`INSERT INTO value_versions (id, entity_id, property_id, space_id, valid_from_key, valid_to_key, text) VALUES ($1,$2,$3,$4,$5,$6,$7)`,
				[crypto.randomUUID(), entity, prop, SPACE, vfrom, vto, text],
			)
		const relVer = (
			relId: string,
			from: string,
			type: string,
			to: string,
			vfrom: number,
			vto: number | null,
			pos: string,
		) =>
			c.query(
				`INSERT INTO relation_versions (id, relation_id, entity_id, type_id, from_entity_id, to_entity_id, space_id, position, valid_from_key, valid_to_key) VALUES ($1,$2,$3,$4,$3,$5,$6,$7,$8,$9)`,
				[crypto.randomUUID(), relId, from, type, to, SPACE, pos, vfrom, vto],
			)

		// media entities: type + url, valid from V1 (immutable, versioned)
		for (const [img, type] of [
			[OLD_IMG, IMAGE_TYPE],
			[NEW_IMG, IMAGE_TYPE],
			[VID, VIDEO_TYPE],
		] as const) {
			await relVer(crypto.randomUUID(), img, TYPES, type, V1, null, "t")
			await valVer(img, IMAGE_URL, V1, null, `https://example.com/${img.slice(-4)}.media`)
		}

		// PAGE value: NAME change (UPDATE)
		await valVer(PAGE, NAME_PROPERTY, V1, V2, "Old Title")
		await valVer(PAGE, NAME_PROPERTY, V2, null, "New Title")

		// cover swap: same relationId, OLD_IMG at V1 → NEW_IMG at V2 (UPDATE → before+after imageUrl)
		await relVer(R_COVER, PAGE, T_COVER, OLD_IMG, V1, V2, "a")
		await relVer(R_COVER, PAGE, T_COVER, NEW_IMG, V2, null, "a")
		// trailer add (video), related-topic add (named target)
		await relVer(crypto.randomUUID(), PAGE, T_TRAILER, VID, V2, null, "b")
		await relVer(crypto.randomUUID(), PAGE, T_TOPIC, TOPIC, V2, null, "c")

		// data block added at V2: Name + Filter values, Collection-item relations.
		// The BLOCKS relation's reified-relation entity is CONFIG_ENTITY, which holds
		// the view/columns config (A2) — must be folded into the data block.
		await relVer(CONFIG_ENTITY, PAGE, BLOCKS, BLOCK_DATA, V2, null, "d")
		await relVer(crypto.randomUUID(), CONFIG_ENTITY, VIEW_PROPERTY, VIEW_TARGET, V2, null, "cfg0")
		await relVer(crypto.randomUUID(), BLOCK_DATA, TYPES, DATA_BLOCK, V2, null, "t")
		await valVer(BLOCK_DATA, NAME_PROPERTY, V2, null, "Benchmarks")
		await valVer(BLOCK_DATA, FILTER_PROP, V2, null, '{"filter":"x"}')
		await relVer(crypto.randomUUID(), BLOCK_DATA, T_COLLECTION_ITEM, TARGET1, V2, null, "i0")
		await relVer(crypto.randomUUID(), BLOCK_DATA, T_COLLECTION_ITEM, TARGET2, V2, null, "i1")

		await c.query("COMMIT")
	} catch (e) {
		await c.query("ROLLBACK")
		throw e
	} finally {
		c.release()
	}
}

async function cleanup(pool: Pool) {
	const ids = ALL_IDS
	await pool.query(
		`DELETE FROM relation_versions WHERE entity_id = ANY($1::uuid[]) OR from_entity_id = ANY($1::uuid[]) OR to_entity_id = ANY($1::uuid[])`,
		[ids],
	)
	await pool.query(`DELETE FROM value_versions WHERE entity_id = ANY($1::uuid[])`, [ids])
	await pool.query(`DELETE FROM "values" WHERE entity_id = ANY($1::uuid[])`, [ids])
	await pool.query(`DELETE FROM edit_versions WHERE edit_id = ANY($1::uuid[])`, [[EDIT1, EDIT2]])
	await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [ids])
}

describe.skipIf(SKIP)("v2 entity-diff enrichment", () => {
	let pool: Pool
	let app: Hono

	beforeAll(async () => {
		pool = new Pool({connectionString: DATABASE_URL})
		app = new Hono()
		app.route("/v2/versioned", createVersionedV2Router(drizzle(pool) as any, runtime))
		await cleanup(pool)
		await seed(pool)
	})
	afterAll(async () => {
		await cleanup(pool)
		await pool?.end()
	})

	const fullDiff = async () => {
		const res = await app.request(`${base}&fromEditId=${EDIT1}&toEditId=${EDIT2}`)
		expect(res.status).toBe(200)
		return res.json() as Promise<any>
	}

	it("resolves names (propertyName / typeName / toEntityName)", async () => {
		const d = await fullDiff()
		const nameVal = d.values.find((v: any) => v.propertyId === normalizeUuid(NAME_PROPERTY))
		expect(nameVal?.propertyName).toBe("Name")
		const topicRel = d.relations.find((r: any) => r.typeId === normalizeUuid(T_TOPIC))
		expect(topicRel?.typeName).toBe("Related topic")
		expect(topicRel?.after?.toEntityName).toBe("Identity Standards")
	})

	it("inlines media URLs with versioned before/after on a cover swap", async () => {
		const d = await fullDiff()
		const cover = d.relations.find((r: any) => r.relationId === normalizeUuid(R_COVER))
		expect(cover?.changeType).toBe("UPDATE")
		// before = OLD_IMG @ V1, after = NEW_IMG @ V2 — each resolved at its own version
		expect(cover?.before?.imageUrl).toContain(OLD_IMG.slice(-4))
		expect(cover?.after?.imageUrl).toContain(NEW_IMG.slice(-4))
	})

	it("inlines videoUrl on a video relation", async () => {
		const d = await fullDiff()
		const trailer = d.relations.find((r: any) => r.typeId === normalizeUuid(T_TRAILER))
		expect(trailer?.after?.videoUrl).toContain(VID.slice(-4))
		expect(trailer?.after?.imageUrl).toBeUndefined()
	})

	it("emits the rich data-block shape (blockName + values + relations w/ spaceId)", async () => {
		const d = await fullDiff()
		const block = d.blocks.find((b: any) => b.id === normalizeUuid(BLOCK_DATA))
		expect(block?.type).toBe("dataBlock")
		expect(block?.blockName).toBe("Benchmarks")
		// Filter value folded in (NAME excluded → blockName/before/after)
		const filter = block?.values?.find((v: any) => v.propertyId === normalizeUuid(FILTER_PROP))
		expect(filter?.propertyName).toBe("Filter")
		// Collection-item relations folded in, with spaceId + resolved names; TYPES stripped
		const items = block?.relations?.filter((r: any) => r.typeId === normalizeUuid(T_COLLECTION_ITEM))
		expect(items?.length).toBe(2)
		expect(block?.relations?.every((r: any) => r.spaceId === normalizeUuid(SPACE))).toBe(true)
		expect(block?.relations?.some((r: any) => r.after?.toEntityName === "Target One")).toBe(true)
		expect(block?.relations?.some((r: any) => r.typeId === normalizeUuid(TYPES))).toBe(false)
	})

	it("merges data-block config (view/columns) from the reified BLOCKS relation entity (A2)", async () => {
		const d = await fullDiff()
		const block = d.blocks.find((b: any) => b.id === normalizeUuid(BLOCK_DATA))
		// VIEW_PROPERTY config lives on the BLOCKS relation entity, not the block —
		// it must be folded into block.relations.
		const viewRel = block?.relations?.find((r: any) => r.typeId === normalizeUuid(VIEW_PROPERTY))
		expect(viewRel).toBeDefined()
		expect(viewRel?.after?.toEntityName).toBe("Gallery View")
	})

	it("supports snapshot mode (no fromEditId → all-added)", async () => {
		const res = await app.request(`${base}&toEditId=${EDIT2}`)
		expect(res.status).toBe(200)
		const d = (await res.json()) as any
		expect(d.fromEditName).toBeNull()
		expect(d.values.every((v: any) => v.before === null)).toBe(true)
		const nameVal = d.values.find((v: any) => v.propertyId === normalizeUuid(NAME_PROPERTY))
		expect(nameVal?.after).toBe("New Title")
		expect(nameVal?.before).toBeNull()
	})

	it("400s when toEditId is missing", async () => {
		const res = await app.request(`${base}`)
		expect(res.status).toBe(400)
	})
})
