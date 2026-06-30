import {Pool} from "pg"
import {afterAll, beforeAll, describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"

// entities_ordered_by_property must be able to return entities that match the
// space/type filters but have NO usable value for the ordered property, via the opt-in
// `includeWithoutValue` flag. Per product guidance, for numeric properties a missing value
// is treated as zero (DESC: positives -> zero/null -> negatives; ASC the reverse); without
// the flag the prior behavior (value-less entities excluded) is preserved.

async function executeGraphQL(query: string, variables?: Record<string, unknown>) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({query, variables}),
		}),
		{},
	)
	return response.json()
}

const undash = (uuid: string) => uuid.replace(/-/g, "")

const TYPES_RELATION_ID = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"

// Well-known fixture UUIDs, chosen so they can't collide with real data and so the
// entity ids sort in a known order (the function tie-breaks ties on e.id ASC).
const SPACE = "00000000-cccc-4ccc-cccc-000000000001"
const TYPE_ID = "00000000-cccc-4ccc-cccc-0000000000aa"
const PROPERTY_ID = "00000000-cccc-4ccc-cccc-0000000000bb"
// Separate properties to exercise the non-numeric (text) and other-numeric (float) branches.
const PROPERTY_TEXT = "00000000-cccc-4ccc-cccc-0000000000cc"
const PROPERTY_FLOAT = "00000000-cccc-4ccc-cccc-0000000000dd"

// integer = 100
const E_POS = "00000000-dddd-4ddd-dddd-000000000001"
// integer = 0
const E_ZERO = "00000000-dddd-4ddd-dddd-000000000002"
// integer = -50
const E_NEG = "00000000-dddd-4ddd-dddd-000000000003"
// no value row for PROPERTY_ID
const E_NOVAL_1 = "00000000-dddd-4ddd-dddd-000000000004"
const E_NOVAL_2 = "00000000-dddd-4ddd-dddd-000000000005"

const ALL_ENTITY_IDS = [E_POS, E_ZERO, E_NEG, E_NOVAL_1, E_NOVAL_2]

// Relation rows are themselves entities; give each its own id (= the subject entity id
// here is fine since we only key cleanup off from_entity_id / entity_id).
const REL_IDS = [
	"00000000-eeee-4eee-eeee-000000000001",
	"00000000-eeee-4eee-eeee-000000000002",
	"00000000-eeee-4eee-eeee-000000000003",
	"00000000-eeee-4eee-eeee-000000000004",
	"00000000-eeee-4eee-eeee-000000000005",
]

async function cleanupFixtures(pool: Pool) {
	await pool.query(`DELETE FROM "values" WHERE entity_id = ANY($1::uuid[])`, [ALL_ENTITY_IDS])
	await pool.query(`DELETE FROM relations WHERE from_entity_id = ANY($1::uuid[]) OR id = ANY($2::uuid[])`, [
		ALL_ENTITY_IDS,
		REL_IDS,
	])
	await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [ALL_ENTITY_IDS])
}

async function seedFixtures(pool: Pool) {
	await pool.query(
		`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
		 SELECT unnest($1::uuid[]), '0', '0', '0', '0'`,
		[ALL_ENTITY_IDS],
	)

	// All five entities are of TYPE_ID in SPACE via a TYPES relation. This is what makes
	// the value-less entities discoverable when includeWithoutValue is true.
	await pool.query(
		`INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id)
		 SELECT unnest($1::uuid[]), unnest($2::uuid[]), $3, unnest($2::uuid[]), $4, $5`,
		[REL_IDS, ALL_ENTITY_IDS, TYPES_RELATION_ID, TYPE_ID, SPACE],
	)

	// Integer values for three of the five entities: positive, zero, negative.
	await pool.query(
		`INSERT INTO "values" (id, property_id, entity_id, space_id, integer) VALUES
			($1, $4, $5, $6, 100),
			($2, $4, $7, $6, 0),
			($3, $4, $8, $6, -50)`,
		[`geo696-${E_POS}`, `geo696-${E_ZERO}`, `geo696-${E_NEG}`, PROPERTY_ID, E_POS, SPACE, E_ZERO, E_NEG],
	)

	// Text values on a separate property — non-numeric, so value-less entities sort NULLS LAST.
	await pool.query(
		`INSERT INTO "values" (id, property_id, entity_id, space_id, text) VALUES
			($1, $4, $5, $7, 'banana'),
			($2, $4, $6, $7, 'apple'),
			($3, $4, $8, $7, 'cherry')`,
		[
			`geo696-text-${E_POS}`,
			`geo696-text-${E_ZERO}`,
			`geo696-text-${E_NEG}`,
			PROPERTY_TEXT,
			E_POS,
			E_ZERO,
			SPACE,
			E_NEG,
		],
	)

	// Float values on a separate property — numeric, so a missing value sorts as zero.
	await pool.query(
		`INSERT INTO "values" (id, property_id, entity_id, space_id, float) VALUES
			($1, $4, $5, $7, 2.5),
			($2, $4, $6, $7, 0),
			($3, $4, $8, $7, -1.5)`,
		[
			`geo696-float-${E_POS}`,
			`geo696-float-${E_ZERO}`,
			`geo696-float-${E_NEG}`,
			PROPERTY_FLOAT,
			E_POS,
			E_ZERO,
			SPACE,
			E_NEG,
		],
	)
}

const ORDER_QUERY = `
	query Q($propertyId: UUID!, $spaceIds: [UUID!], $typeIds: [UUID!], $dir: SortOrder!, $include: Boolean) {
		entitiesOrderedByProperty(
			propertyId: $propertyId
			spaceIds: $spaceIds
			typeIds: $typeIds
			dataType: "integer"
			sortDirection: $dir
			includeWithoutValue: $include
			first: 50
		) {
			id
		}
	}
`

// Same as ORDER_QUERY but with a parameterized dataType, for the text/float cases.
const ORDER_QUERY_DT = `
	query Q($propertyId: UUID!, $spaceIds: [UUID!], $typeIds: [UUID!], $dataType: String!, $dir: SortOrder!, $include: Boolean) {
		entitiesOrderedByProperty(
			propertyId: $propertyId
			spaceIds: $spaceIds
			typeIds: $typeIds
			dataType: $dataType
			sortDirection: $dir
			includeWithoutValue: $include
			first: 50
		) {
			id
		}
	}
`

function idsOf(result: {data?: {entitiesOrderedByProperty?: Array<{id: string}>}}): string[] {
	return (result.data?.entitiesOrderedByProperty ?? []).map((e) => e.id)
}

const baseVars = {
	propertyId: undash(PROPERTY_ID),
	spaceIds: [undash(SPACE)],
	typeIds: [undash(TYPE_ID)],
}

describe("entitiesOrderedByProperty — include entities without a value", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		// Idempotent cleanup in case a prior run aborted between seed and teardown.
		await cleanupFixtures(pool)
		await seedFixtures(pool)
	})

	afterAll(async () => {
		if (pool) {
			await cleanupFixtures(pool)
			await pool.end()
		}
	})

	it("exposes includeWithoutValue as a Boolean argument on both fields", async () => {
		const result = await executeGraphQL(`
			{
				__type(name: "Query") {
					fields { name args { name type { name kind ofType { name } } } }
				}
			}
		`)
		expect(result.errors).toBeUndefined()

		for (const fieldName of ["entitiesOrderedByProperty", "entitiesOrderedByPropertyConnection"]) {
			const field = result.data.__type.fields.find((f: {name: string}) => f.name === fieldName)
			expect(field, `${fieldName} should exist`).toBeDefined()
			const arg = field.args.find((a: {name: string}) => a.name === "includeWithoutValue")
			expect(arg, `${fieldName}.includeWithoutValue should exist`).toBeDefined()
			const typeName = arg.type.name ?? arg.type.ofType?.name
			expect(typeName).toBe("Boolean")
		}
	})

	it("excludes value-less entities by default (preserves prior behavior)", async () => {
		const result = await executeGraphQL(ORDER_QUERY, {...baseVars, dir: "DESC", include: false})
		expect(result.errors).toBeUndefined()
		const ids = idsOf(result)
		// Only the three entities that have an integer value, ordered DESC: 100, 0, -50.
		expect(ids).toEqual([undash(E_POS), undash(E_ZERO), undash(E_NEG)])
		expect(ids).not.toContain(undash(E_NOVAL_1))
		expect(ids).not.toContain(undash(E_NOVAL_2))
	})

	it("omitting includeWithoutValue behaves the same as false", async () => {
		const result = await executeGraphQL(
			`
				query Q($propertyId: UUID!, $spaceIds: [UUID!], $typeIds: [UUID!]) {
					entitiesOrderedByProperty(
						propertyId: $propertyId
						spaceIds: $spaceIds
						typeIds: $typeIds
						dataType: "integer"
						sortDirection: DESC
						first: 50
					) { id }
				}
			`,
			baseVars,
		)
		expect(result.errors).toBeUndefined()
		expect(idsOf(result)).toEqual([undash(E_POS), undash(E_ZERO), undash(E_NEG)])
	})

	it("DESC: includes value-less entities at the zero position (positives -> zero/null -> negatives)", async () => {
		const result = await executeGraphQL(ORDER_QUERY, {...baseVars, dir: "DESC", include: true})
		expect(result.errors).toBeUndefined()
		// 100 first; then the zero group {E_ZERO, E_NOVAL_1, E_NOVAL_2} ordered by id ASC
		// (null treated as zero); then -50 last.
		expect(idsOf(result)).toEqual([
			undash(E_POS),
			undash(E_ZERO),
			undash(E_NOVAL_1),
			undash(E_NOVAL_2),
			undash(E_NEG),
		])
	})

	it("ASC: includes value-less entities at the zero position (negatives -> zero/null -> positives)", async () => {
		const result = await executeGraphQL(ORDER_QUERY, {...baseVars, dir: "ASC", include: true})
		expect(result.errors).toBeUndefined()
		expect(idsOf(result)).toEqual([
			undash(E_NEG),
			undash(E_ZERO),
			undash(E_NOVAL_1),
			undash(E_NOVAL_2),
			undash(E_POS),
		])
	})

	it("treats an explicit includeWithoutValue: null the same as false (excludes value-less)", async () => {
		// PostGraphile exposes includeWithoutValue as a nullable Boolean, so a client can pass
		// null explicitly. With typeIds present, null must NOT opt into the unscored branch.
		const result = await executeGraphQL(ORDER_QUERY, {...baseVars, dir: "DESC", include: null})
		expect(result.errors).toBeUndefined()
		const ids = idsOf(result)
		expect(ids).toEqual([undash(E_POS), undash(E_ZERO), undash(E_NEG)])
		expect(ids).not.toContain(undash(E_NOVAL_1))
		expect(ids).not.toContain(undash(E_NOVAL_2))
	})

	it("falls back to scored-only when includeWithoutValue is true but no typeIds are given", async () => {
		const result = await executeGraphQL(
			`
				query Q($propertyId: UUID!, $spaceIds: [UUID!]) {
					entitiesOrderedByProperty(
						propertyId: $propertyId
						spaceIds: $spaceIds
						dataType: "integer"
						sortDirection: DESC
						includeWithoutValue: true
						first: 50
					) { id }
				}
			`,
			{propertyId: undash(PROPERTY_ID), spaceIds: [undash(SPACE)]},
		)
		expect(result.errors).toBeUndefined()
		const ids = idsOf(result)
		expect(ids).toEqual([undash(E_POS), undash(E_ZERO), undash(E_NEG)])
	})

	it("text (non-numeric): value-less entities sort to the tail via NULLS LAST, DESC", async () => {
		const result = await executeGraphQL(ORDER_QUERY_DT, {
			propertyId: undash(PROPERTY_TEXT),
			spaceIds: [undash(SPACE)],
			typeIds: [undash(TYPE_ID)],
			dataType: "text",
			dir: "DESC",
			include: true,
		})
		expect(result.errors).toBeUndefined()
		// DESC: cherry > banana > apple, then value-less entities last (NULLS LAST).
		expect(idsOf(result)).toEqual([
			undash(E_NEG),
			undash(E_POS),
			undash(E_ZERO),
			undash(E_NOVAL_1),
			undash(E_NOVAL_2),
		])
	})

	it("text (non-numeric): value-less entities sort to the tail via NULLS LAST, ASC", async () => {
		const result = await executeGraphQL(ORDER_QUERY_DT, {
			propertyId: undash(PROPERTY_TEXT),
			spaceIds: [undash(SPACE)],
			typeIds: [undash(TYPE_ID)],
			dataType: "text",
			dir: "ASC",
			include: true,
		})
		expect(result.errors).toBeUndefined()
		// ASC: apple < banana < cherry, then value-less last (NULLS LAST in both directions).
		expect(idsOf(result)).toEqual([
			undash(E_ZERO),
			undash(E_POS),
			undash(E_NEG),
			undash(E_NOVAL_1),
			undash(E_NOVAL_2),
		])
	})

	it("float (numeric): a missing value sorts as zero, DESC", async () => {
		const result = await executeGraphQL(ORDER_QUERY_DT, {
			propertyId: undash(PROPERTY_FLOAT),
			spaceIds: [undash(SPACE)],
			typeIds: [undash(TYPE_ID)],
			dataType: "float",
			dir: "DESC",
			include: true,
		})
		expect(result.errors).toBeUndefined()
		// 2.5 first; then the zero group {0, value-less} by id; then -1.5.
		expect(idsOf(result)).toEqual([
			undash(E_POS),
			undash(E_ZERO),
			undash(E_NOVAL_1),
			undash(E_NOVAL_2),
			undash(E_NEG),
		])
	})

	it("raises when includeWithoutValue is true but no spaceIds are given", async () => {
		const result = await executeGraphQL(
			`
				query Q($propertyId: UUID!, $typeIds: [UUID!]) {
					entitiesOrderedByProperty(
						propertyId: $propertyId
						typeIds: $typeIds
						dataType: "integer"
						sortDirection: DESC
						includeWithoutValue: true
						first: 50
					) { id }
				}
			`,
			{propertyId: undash(PROPERTY_ID), typeIds: [undash(TYPE_ID)]},
		)
		// space_ids is mandatory when includeWithoutValue is true (bounds the candidate scan).
		expect(result.errors).toBeDefined()
		expect(result.data?.entitiesOrderedByProperty ?? null).toBeFalsy()
	})
})

// An entity is "scored" only when its value and its TYPES relation live in the SAME space.
// An entity valued in one space but typed in another is therefore not scored in any single
// space and is intentionally treated as value-less (sorts at the zero position). Isolated
// fixtures (distinct UUIDs) so this multi-space layout can't perturb the single-space suite.
const MS_SPACE_A = "00000000-c2cc-4ccc-cccc-00000000000a"
const MS_SPACE_B = "00000000-c2cc-4ccc-cccc-00000000000b"
const MS_TYPE = "00000000-c2cc-4ccc-cccc-0000000000aa"
const MS_PROP = "00000000-c2cc-4ccc-cccc-0000000000bb"
const MS_HI = "00000000-c2dd-4ddd-dddd-000000000001" // integer 50, typed + valued in SPACE_A
const MS_CROSS = "00000000-c2dd-4ddd-dddd-000000000002" // integer 999 in SPACE_A, typed in SPACE_B
const MS_LO = "00000000-c2dd-4ddd-dddd-000000000003" // integer -50, typed + valued in SPACE_A
const MS_ENTITY_IDS = [MS_HI, MS_CROSS, MS_LO]

async function cleanupMs(pool: Pool) {
	await pool.query(`DELETE FROM "values" WHERE entity_id = ANY($1::uuid[])`, [MS_ENTITY_IDS])
	await pool.query(`DELETE FROM relations WHERE from_entity_id = ANY($1::uuid[])`, [MS_ENTITY_IDS])
	await pool.query(`DELETE FROM entities WHERE id = ANY($1::uuid[])`, [MS_ENTITY_IDS])
}

describe("entitiesOrderedByProperty — multi-space scoping", () => {
	let pool: Pool

	beforeAll(async () => {
		pool = new Pool({connectionString: process.env.DATABASE_URL})
		await cleanupMs(pool)
		await pool.query(
			`INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
			 SELECT unnest($1::uuid[]), '0', '0', '0', '0'`,
			[MS_ENTITY_IDS],
		)
		// MS_HI / MS_LO are typed in SPACE_A; MS_CROSS is typed in SPACE_B.
		await pool.query(
			`INSERT INTO relations (id, entity_id, type_id, from_entity_id, to_entity_id, space_id) VALUES
				($1, $1, $4, $1, $5, $6),
				($2, $2, $4, $2, $5, $7),
				($3, $3, $4, $3, $5, $6)`,
			[MS_HI, MS_CROSS, MS_LO, TYPES_RELATION_ID, MS_TYPE, MS_SPACE_A, MS_SPACE_B],
		)
		// All three carry their integer value in SPACE_A.
		await pool.query(
			`INSERT INTO "values" (id, property_id, entity_id, space_id, integer) VALUES
				($1, $4, $5, $6, 50),
				($2, $4, $7, $6, 999),
				($3, $4, $8, $6, -50)`,
			["geo696-ms-hi", "geo696-ms-cross", "geo696-ms-lo", MS_PROP, MS_HI, MS_SPACE_A, MS_CROSS, MS_LO],
		)
	})

	afterAll(async () => {
		if (pool) {
			await cleanupMs(pool)
			await pool.end()
		}
	})

	it("treats an entity valued in one space but typed in another as value-less (sorts at zero)", async () => {
		const result = await executeGraphQL(ORDER_QUERY_DT, {
			propertyId: undash(MS_PROP),
			spaceIds: [undash(MS_SPACE_A), undash(MS_SPACE_B)],
			typeIds: [undash(MS_TYPE)],
			dataType: "integer",
			dir: "DESC",
			include: true,
		})
		expect(result.errors).toBeUndefined()
		// MS_HI (50) is scored. MS_CROSS holds integer 999 in SPACE_A but its TYPES relation is
		// in SPACE_B, so it is not scored in any single space and sorts at zero — above MS_LO
		// (-50) and below MS_HI — despite its value.
		expect(idsOf(result)).toEqual([undash(MS_HI), undash(MS_CROSS), undash(MS_LO)])
	})
})
