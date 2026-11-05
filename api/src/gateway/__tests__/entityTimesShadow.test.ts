import {readFileSync} from "node:fs"
import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {
	CREATED_AT_PROPERTY,
	conformingId,
	diffCards,
	ENTITY_TIME_PROPERTIES,
	entityTimesOf,
	type GeoCardRow,
	isEmptyDiff,
	rowsFromGeoDbOutput,
	UPDATED_AT_PROPERTY,
	type ValueRow,
	valueKey,
} from "../../geodb/card"
import {
	analyzeEntityOperation,
	compareDerived,
	createEntityShadow,
	type EntityPlan,
	extractPostgresCard,
	type PostgresCard,
	secondsOrNull,
} from "../entityShadow"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"

// The captured Entity query selects `updatedAt` (PostGraphile answered "1793337008"); `createdAt` is added here.
const fixture = JSON.parse(readFileSync(new URL("./fixtures/entity-space-a.json", import.meta.url), "utf8")) as {
	request: {query: string; operationName: string; variables: Record<string, string>}
	response: {data: {entity: Record<string, any>}}
}
const ENTITY_HEX = "073ff16be7fe4591a984c325d629d92d"
const SPACE_A = "41e851610e13a19441c4d980f2f2ce6b"
const GEO_SPACE_A = "aaaaaaaaaaaa4aaa8aaaaaaaaaaaaaaa"
const PG_UPDATED = 1793337008
const PG_CREATED = 1790000000

const QUERY = fixture.request.query.replace("updatedAt", "updatedAt\n    createdAt")
const withCreated = () => {
	const response = structuredClone(fixture.response)
	response.data.entity.createdAt = String(PG_CREATED)
	return response
}

const cell = (v: string) => v.replaceAll("\\", "\\\\").replaceAll("\t", "\\t").replaceAll("\n", "\\n")
const instantRaw = (seconds: number) => `instant:${seconds}.000000000`

/** The card output of the fixture's rows (names on), as entityShadowFields.test.ts builds it, plus `extra` rows. */
function geoOutput(extra: {prop: string; raw: string}[] = []): string {
	const entity = fixture.response.data.entity
	const rows: string[] = []
	let n = 0
	const tail = (a: string | null | undefined, b: string | null | undefined) =>
		`\t${a ? cell(a) : "-"}\t${b ? cell(b) : "-"}\t1`
	for (const v of entity.valuesList) {
		const row: ValueRow = {
			property_id: v.property.id,
			text: v.text,
			integer: v.integer,
			float: v.float,
			decimal: v.decimal,
			boolean: v.boolean,
			datetime: v.datetime,
			date: v.date,
		}
		const key = valueKey(row)
		if (!key) throw new Error("fixture value not mappable")
		rows.push(
			`${conformingId("entity", v.property.id)}\t${cell(key.norm)}\ts${n++}\t${ENTITY_HEX}${tail(v.property.name, null)}`,
		)
	}
	for (const r of entity.relations?.nodes ?? entity.relationsList)
		rows.push(
			`${conformingId("entity", r.type.id)}\t${conformingId("entity", r.toEntity.id)}\ts${n++}\t${ENTITY_HEX}${tail(r.type.name, r.toEntity.name)}`,
		)
	for (const e of extra) rows.push(`${e.prop}\t${e.raw}\ts${n++}\t${ENTITY_HEX}${tail(null, null)}`)
	return `${rows.join("\n")}\n`
}

const time = (property: string, seconds: number) => ({prop: property, raw: instantRaw(seconds)})

function setup(extra: {prop: string; raw: string}[]) {
	const output = geoOutput(extra)
	const log = {info: vi.fn(), warn: vi.fn()}
	const shadow = createEntityShadow({
		geoDb: {
			readCard: vi.fn(async ({entityId, names}: {geoSpaceId: string; entityId: string; names?: boolean}) =>
				rowsFromGeoDbOutput(output, entityId, {names}),
			),
		},
		geoSpaceOf: async (space) => (space === SPACE_A ? GEO_SPACE_A : null),
		log,
	})
	return {shadow, log}
}

async function run(extra: {prop: string; raw: string}[], query = fixture.request.query, response = fixture.response) {
	const {shadow, log} = setup(extra)
	await shadow({
		classification: {operationName: fixture.request.operationName, operationType: "query", rootFields: ["entity"]},
		params: {query, operationName: fixture.request.operationName, variables: fixture.request.variables},
		document: parse(query),
		response: Response.json(response),
	})
	return log
}

const counts = (prefix: string) =>
	Object.fromEntries(
		[
			...renderGatewayMetrics().matchAll(
				new RegExp(`${prefix}\\{[^}]*?(?:result|reason)="([^"]+)"[^}]*\\} (\\d+)`, "g"),
			),
		].map((m) => [m[1], Number(m[2])]),
	)
const results = () => counts("gateway_shadow_total")
const uncompared = () => counts("gateway_shadow_uncompared_total")

beforeEach(() => __resetGatewayMetricsForTests())

describe("the reserved time properties", () => {
	it("are the ids the feeder mints by name", () => {
		const mint = (name: string) => conformingId("reserved-property" as never, name)
		expect(CREATED_AT_PROPERTY).toBe(mint("createdAt"))
		expect(UPDATED_AT_PROPERTY).toBe(mint("updatedAt"))
		expect(ENTITY_TIME_PROPERTIES.size).toBe(2)
	})

	it("stay equal to the geodb-feeder constants", () => {
		const src = readFileSync(
			new URL("../../../../geodb-feeder/src/__tests__/entityTimes.test.ts", import.meta.url),
			"utf8",
		)
		expect(src).toContain(`"${CREATED_AT_PROPERTY}"`)
		expect(src).toContain(`"${UPDATED_AT_PROPERTY}"`)
	})

	it("are kept out of the card diff, which counts no geo-db-only row for them", () => {
		const geo: GeoCardRow[] = [time(CREATED_AT_PROPERTY, 1), time(UPDATED_AT_PROPERTY, 2)].map((r) => ({...r}))
		expect(isEmptyDiff(diffCards({statements: [], skipped: 0}, geo))).toBe(true)
	})

	it("read as the least createdAt and greatest updatedAt over the Spaces", () => {
		const rows = [
			time(CREATED_AT_PROPERTY, 300),
			time(CREATED_AT_PROPERTY, 100),
			time(UPDATED_AT_PROPERTY, 400),
			time(UPDATED_AT_PROPERTY, 900),
			{prop: UPDATED_AT_PROPERTY, raw: "not an instant"},
		]
		expect(entityTimesOf(rows)).toEqual({createdAt: 100, updatedAt: 900})
		expect(entityTimesOf([])).toEqual({createdAt: null, updatedAt: null})
	})

	it("PostGraphile's seconds text parses, an ISO instant too, nothing else", () => {
		expect(secondsOrNull("1793337008")).toBe(1793337008)
		expect(secondsOrNull("2026-10-02T00:00:00Z")).toBe(1790899200)
		expect(secondsOrNull(null)).toBeNull()
		expect(secondsOrNull("later")).toBeNull()
	})
})

describe("compareDerived on updatedAt and createdAt", () => {
	const plan = (): EntityPlan => {
		const r = analyzeEntityOperation(parse(QUERY), fixture.request.operationName, fixture.request.variables)
		if (!r.ok) throw new Error(r.reason)
		return r.plan
	}
	const check = (geo: {prop: string; raw: string}[], pg: {updatedAt?: number | null; createdAt?: number | null}) => {
		const p = plan()
		const card = {
			...extractPostgresCard({data: {[p.key]: {...fixture.response.data.entity, createdAt: "1"}}}, p),
			...pg,
		} as NonNullable<PostgresCard>
		return compareDerived(p, card, geo)
	}

	it("plans both fields, with their response keys", () => {
		expect(plan().updatedAt).toBe("updatedAt")
		expect(plan().createdAt).toBe("createdAt")
	})

	it("equal is ok", () => {
		const r = check([time(UPDATED_AT_PROPERTY, PG_UPDATED), time(CREATED_AT_PROPERTY, PG_CREATED)], {
			updatedAt: PG_UPDATED,
			createdAt: PG_CREATED,
		})
		expect(r.checks).toMatchObject({updatedAt: "ok", createdAt: "ok"})
	})

	it("geo-db behind Postgres on updatedAt (it stamps fewer touches) is inconclusive, never a mismatch", () => {
		const r = check([time(UPDATED_AT_PROPERTY, PG_UPDATED - 50)], {updatedAt: PG_UPDATED, createdAt: null})
		expect(r.checks.updatedAt).toBe("inconclusive")
		expect(r.uncompared).toContain("updated_at")
	})

	it("geo-db ahead of Postgres on updatedAt is a mismatch", () => {
		const r = check([time(UPDATED_AT_PROPERTY, PG_UPDATED + 1)], {updatedAt: PG_UPDATED, createdAt: null})
		expect(r.checks.updatedAt).toBe("mismatch")
	})

	it("createdAt: later in geo-db is inconclusive, earlier is a mismatch", () => {
		expect(
			check([time(CREATED_AT_PROPERTY, PG_CREATED + 5)], {createdAt: PG_CREATED, updatedAt: null}).checks
				.createdAt,
		).toBe("inconclusive")
		expect(
			check([time(CREATED_AT_PROPERTY, PG_CREATED - 5)], {createdAt: PG_CREATED, updatedAt: null}).checks
				.createdAt,
		).toBe("mismatch")
	})

	it("two Spaces: the greatest updatedAt and least createdAt decide", () => {
		const geo = [
			time(UPDATED_AT_PROPERTY, PG_UPDATED - 10),
			time(UPDATED_AT_PROPERTY, PG_UPDATED),
			time(CREATED_AT_PROPERTY, PG_CREATED + 10),
			time(CREATED_AT_PROPERTY, PG_CREATED),
		]
		expect(check(geo, {updatedAt: PG_UPDATED, createdAt: PG_CREATED}).checks).toMatchObject({
			updatedAt: "ok",
			createdAt: "ok",
		})
	})

	it("an entity with no time in geo-db yet (feeder off, backfill not run) is uncompared, and an absent Postgres time is skipped", () => {
		const r = check([], {updatedAt: PG_UPDATED, createdAt: null})
		expect(r.checks.updatedAt).toBe("inconclusive")
		expect(r.checks.createdAt).toBeUndefined()
		expect(r.uncompared).toContain("updated_at")
	})
})

describe("the entity shadow end to end", () => {
	it("updatedAt equal: match, and updatedAt is no longer counted uncompared", async () => {
		const log = await run([time(UPDATED_AT_PROPERTY, PG_UPDATED)])
		expect(results()).toMatchObject({match: 1, mismatch: 0})
		expect(uncompared().updated_at ?? 0).toBe(0)
		expect(log.warn).not.toHaveBeenCalled()
	})

	it("no time rows in geo-db: still a match, with updatedAt uncompared", async () => {
		await run([])
		expect(results()).toMatchObject({match: 1, mismatch: 0})
		expect(uncompared().updated_at).toBe(1)
	})

	it("updatedAt ahead of Postgres: a mismatch, logged under derivedMismatches", async () => {
		const log = await run([time(UPDATED_AT_PROPERTY, PG_UPDATED + 60)])
		expect(results()).toMatchObject({match: 0, mismatch: 1})
		expect(log.warn).toHaveBeenCalledWith(
			"gateway entity shadow mismatch",
			expect.objectContaining({derivedMismatches: ["updatedAt"], onlyGeoDb: 0}),
		)
	})

	it("both fields compared when both are selected", async () => {
		await run([time(UPDATED_AT_PROPERTY, PG_UPDATED), time(CREATED_AT_PROPERTY, PG_CREATED)], QUERY, withCreated())
		expect(results()).toMatchObject({match: 1, mismatch: 0})
		expect(uncompared().created_at ?? 0).toBe(0)
	})

	it("createdAt earlier in geo-db than in Postgres: a mismatch", async () => {
		await run(
			[time(UPDATED_AT_PROPERTY, PG_UPDATED), time(CREATED_AT_PROPERTY, PG_CREATED - 1)],
			QUERY,
			withCreated(),
		)
		expect(results()).toMatchObject({match: 0, mismatch: 1})
	})
})
