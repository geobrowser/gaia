import {readFileSync} from "node:fs"
import {parse} from "graphql"
import {beforeEach, describe, expect, it, vi} from "vitest"
import {backlinkRowsFromGeoDbOutput, conformingId} from "../../geodb/card"
import {analyzeBacklinks, BACKLINKS_LIMIT} from "../backlinksShadow"
import {createEntityShadow} from "../entityShadow"
import {__resetGatewayMetricsForTests, renderGatewayMetrics} from "../metrics"
import {createReadBudget} from "../readBudget"

// geogenesis's EntityBacklinksPage and PostGraphile's answer, captured read-only from the testnet gaia API: a
// claim referred to by a News story and a Data block, both in one Space. The geo-db side is built below in
// geodb-client's card-with-backlinks output format.
type Row = {spaceId: string; fromEntity: {id: string; name: string} | null}
type Fixture = {
	request: {query: string; variables: Record<string, unknown>}
	response: {data: {entity: {backlinksList: Row[]}}}
}
const FIXTURE: Fixture = JSON.parse(readFileSync(new URL("./fixtures/entity-backlinks.json", import.meta.url), "utf8"))
const QUERY = FIXTURE.request.query
const ENTITY = (FIXTURE.request.variables.id as string).replaceAll("-", "")
const ROWS = FIXTURE.response.data.entity.backlinksList
const SPACE = ROWS[0]?.spaceId as string
const OTHER = "5093b6ab1b48f1111dd31275b5413ddb"
const UNMAPPED = "0123456789ab4bcd8ef0123456789abc"
const geoSpace = (s: string) => `${s.slice(0, 12)}4${s.slice(13, 16)}8${s.slice(17)}`
const HEX = conformingId("entity", ENTITY)
const RELATED = conformingId("entity", "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1")
const MEMBER = "dbe3bdf637a5458d9206d737fb104793"

/** A backlink row as geodb-client prints it: `from` holds `prop` with the entity as value. */
const ref = (from: string, prop = RELATED, n = 0) => `${prop}\t${HEX}\ts${n}\t${conformingId("entity", from)}\t1\n`
/** The fixture's referrers in geo-db's form. */
const fixtureRefs = () => ROWS.map((r, i) => ref(r.fromEntity?.id as string, RELATED, i)).join("")

const series = (prefix: string) =>
	Object.fromEntries(
		renderGatewayMetrics()
			.split("\n")
			.filter((l) => l.startsWith(`${prefix}{`) && !l.endsWith(" 0"))
			.map((l) => [l.match(/(?:result|reason)="(\w+)"/)?.[1], Number(l.split(" ").pop())]),
	)
const results = () => series("gaia_api_gateway_shadow_total")
const unsupported = () => series("gaia_api_gateway_shadow_unsupported_total")
const uncompared = () => series("gaia_api_gateway_shadow_uncompared_total")

beforeEach(() => __resetGatewayMetricsForTests())

/**
 * An entity shadow whose geo-db answers backlinks from `outputs` (one card output per geo-db Space), printing the
 * multi-Space `<space>:<statement>` form when the line names several Spaces.
 */
function setup(
	outputs: Record<string, string>,
	options: {
		flag?: boolean
		every?: boolean
		budget?: ReturnType<typeof createReadBudget>
		spaces?: string[]
		chunks?: boolean
	} = {},
) {
	const log = {info: vi.fn(), warn: vi.fn()}
	const readBacklinks = vi.fn(
		async ({geoSpaceIds, entityId}: {geoSpaceIds: string[]; entityId: string; limit: number}) => {
			const spaces = geoSpaceIds.length > 1
			const text = geoSpaceIds
				.flatMap((geo) =>
					(outputs[geo] ?? "")
						.split("\n")
						.filter(Boolean)
						.map((line) =>
							spaces
								? line
										.split("\t")
										.map((c, i) => (i === 2 ? `${geo}:${c}` : c))
										.join("\t")
								: line,
						),
				)
				.join("\n")
			return backlinkRowsFromGeoDbOutput(`${text}\n`, entityId, {spaces})
		},
	)
	const geoDb = {readCard: vi.fn(async () => []), readBacklinks}
	const allGeoSpaces = vi.fn(async () => options.spaces ?? [geoSpace(SPACE), geoSpace(OTHER)])
	const shadow = createEntityShadow({
		multiSpace: true,
		everySpaceChunks: options.chunks,
		backlinks: options.flag !== false,
		geoDb,
		geoSpaceOf: async (space: string) => (space === UNMAPPED ? null : geoSpace(space)),
		log,
		...(options.every === false ? {} : {allGeoSpaces}),
		...(options.budget ? {budget: options.budget, everySpaceBudget: options.budget} : {}),
	})
	return {log, readBacklinks, allGeoSpaces, shadow}
}

const call = (
	shadow: (c: never) => Promise<void>,
	variables: Record<string, unknown>,
	body: unknown = FIXTURE.response,
	query = QUERY,
) =>
	shadow({
		classification: {operationName: null, operationType: "query", rootFields: []},
		params: {query, operationName: null, variables},
		document: parse(query),
		response: Response.json(body),
	} as never)

const withRows = (rows: Row[]) => ({data: {entity: {backlinksList: rows}}})

describe("analyzeBacklinks", () => {
	const plan = (query: string, variables: Record<string, unknown> = {id: ENTITY}) =>
		analyzeBacklinks(parse(query), null, variables)

	it("plans EntityBacklinksPage with or without a Space", () => {
		const every = plan(QUERY, FIXTURE.request.variables)
		expect(every.ok && every.plan).toMatchObject({
			entityId: ENTITY,
			spaceId: null,
			spaceKey: "spaceId",
			uncompared: ["nested_entity"],
		})
		const one = plan(QUERY, {id: ENTITY, spaceId: SPACE})
		expect(one.ok && one.plan.spaceId).toBe(SPACE)
	})

	it("needs only the referrer's id, and compares nothing else of it", () => {
		const bare = plan(`query($id: UUID!) { entity(id: $id) { backlinksList { fromEntity { id } } } }`)
		expect(bare.ok && bare.plan).toMatchObject({spaceId: null, spaceKey: null, uncompared: []})
	})

	it("refuses what a card's backlinks cannot answer", () => {
		const refusals: [string, string][] = [
			[`query($id: UUID!) { entity(id: $id) { name backlinksList { fromEntity { id } } } }`, "selection"],
			[`query($id: UUID!) { entity(id: $id) { backlinksList { fromEntity { name } } } }`, "selection"],
			[`query($id: UUID!) { entity(id: $id) { backlinksList { id fromEntity { id } } } }`, "selection"],
			[`query($id: UUID!) { entity(id: $id) { backlinksList(orderBy: ID_ASC) { fromEntity { id } } } }`, "args"],
			[
				`query($id: UUID!) { entity(id: $id) { backlinksList(filter: {fromEntityId: {is: $id}}) { fromEntity { id } } } }`,
				"filter",
			],
			[
				`query($id: UUID!) { entity(id: $id) { backlinksList(filter: {spaceId: {in: [$id]}}) { fromEntity { id } } } }`,
				"filter",
			],
			[
				`query($id: UUID!) { entity(id: $id) { backlinksList { fromEntity { id } } } e: entity(id: $id) { id } }`,
				"not_single_root",
			],
		]
		for (const [query, reason] of refusals) expect(plan(query)).toEqual({ok: false, reason})
	})
})

describe("the typed, paged backlinks page (EntityCommentReplyBacklinksPage)", () => {
	const TYPED = `query($id: UUID!, $type: UUID!, $first: Int, $offset: Int) { entity(id: $id) { backlinksList(first: $first, offset: $offset, filter: {typeId: {is: $type}, fromEntity: {typeIds: {overlaps: []}}}) { fromEntity { id } } } }`
	const TYPE = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1"
	const OTHER_TYPE = conformingId("entity", "7ed45f2b-c48b-419e-8e46-64d5ff680b0d")
	const vars = {id: ENTITY, type: TYPE, first: 20, offset: 0}

	it("plans the page: relation types, an inexact filter (the referrer's own), paging", () => {
		const a = analyzeBacklinks(parse(TYPED), null, vars)
		expect(a.ok && a.plan).toMatchObject({types: [TYPE.replaceAll("-", "")], exact: false, first: 20, offset: 0})
	})

	const page = (ids: string[]) => ({
		data: {entity: {backlinksList: ids.map((id) => ({fromEntity: {id}}))}},
	})
	it("holds the page's rows to geo-db's referrers of that relation type only", async () => {
		const [a, b] = ROWS.map((r) => r.fromEntity?.id as string)
		const refs = ref(a as string, RELATED, 0) + ref(b as string, OTHER_TYPE, 1)
		const s = setup({[geoSpace(SPACE)]: refs})
		await call(s.shadow as never, vars, page([a as string]), TYPED)
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({filter_completeness: 1})
		// A row geo-db holds only under another relation type is a mismatch.
		const wrong = setup({[geoSpace(SPACE)]: refs})
		await call(wrong.shadow as never, vars, page([b as string]), TYPED)
		expect(results()).toEqual({match: 1, mismatch: 1})
	})
})

describe("entity shadow, backlinks", () => {
	it("matches the captured page against geo-db's backlinks in every Space, in one read", async () => {
		const s = setup({[geoSpace(SPACE)]: fixtureRefs()})
		await call(s.shadow as never, FIXTURE.request.variables)
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toEqual({nested_entity: 1})
		expect(s.readBacklinks).toHaveBeenCalledTimes(1)
		expect(s.readBacklinks.mock.calls[0]?.[0]).toEqual({
			geoSpaceIds: [geoSpace(SPACE), geoSpace(OTHER)],
			entityId: HEX,
			limit: BACKLINKS_LIMIT,
		})
	})

	it("reads only the filter's Space when there is one", async () => {
		const s = setup({[geoSpace(SPACE)]: fixtureRefs()})
		await call(s.shadow as never, {id: ENTITY, spaceId: SPACE})
		expect(results()).toEqual({match: 1})
		expect(s.readBacklinks.mock.calls[0]?.[0].geoSpaceIds).toEqual([geoSpace(SPACE)])
		expect(s.allGeoSpaces).not.toHaveBeenCalled()
	})

	it("is a mismatch when geo-db misses a referrer, or holds one PostGraphile does not list", async () => {
		const first = ROWS[0]?.fromEntity?.id as string
		const s = setup({[geoSpace(SPACE)]: ref(first)})
		await call(s.shadow as never, FIXTURE.request.variables)
		expect(results()).toEqual({mismatch: 1})
		const extra = setup({[geoSpace(SPACE)]: fixtureRefs() + ref("9b1f76ff-9711-404c-861e-59dc3fa7d037")})
		await call(extra.shadow as never, FIXTURE.request.variables)
		expect(results()).toEqual({mismatch: 2})
	})

	it("compares Spaces when the rows say them: the right referrer in the wrong Space is a mismatch", async () => {
		const s = setup({[geoSpace(OTHER)]: fixtureRefs()})
		await call(s.shadow as never, FIXTURE.request.variables)
		expect(results()).toEqual({mismatch: 1})
	})

	it("ignores the membership references only geo-db holds", async () => {
		const s = setup({[geoSpace(SPACE)]: fixtureRefs() + ref("9b1f76ff-9711-404c-861e-59dc3fa7d037", MEMBER, 9)})
		await call(s.shadow as never, FIXTURE.request.variables)
		expect(results()).toEqual({match: 1})
	})

	it("checks only PostGraphile's rows against geo-db when its page was full", async () => {
		const page = Array.from({length: 100}, (_, i) => ({
			spaceId: SPACE,
			fromEntity: {id: `0000000000004000800000000000${String(i).padStart(4, "0")}`, name: "x"},
		}))
		const refs = page.map((r, i) => ref(r.fromEntity.id, RELATED, i)).join("")
		const more = ref("9b1f76ff-9711-404c-861e-59dc3fa7d037", RELATED, 200)
		const s = setup({[geoSpace(SPACE)]: refs + more})
		await call(s.shadow as never, FIXTURE.request.variables, withRows(page))
		expect(results()).toEqual({match: 1})
		expect(uncompared()).toMatchObject({truncated: 1})
	})

	it("counts a row in a Space geo-db has not loaded as unsupported", async () => {
		const rows = [...ROWS, {spaceId: UNMAPPED, fromEntity: {id: "9b1f76ff9711404c861e59dc3fa7d037", name: "x"}}]
		const s = setup({[geoSpace(SPACE)]: fixtureRefs()})
		await call(s.shadow as never, FIXTURE.request.variables, withRows(rows))
		expect(results()).toEqual({unsupported_selection: 1})
		expect(unsupported()).toEqual({unmapped_space: 1})
		await call(s.shadow as never, {id: ENTITY, spaceId: UNMAPPED})
		expect(unsupported()).toEqual({unmapped_space: 2})
	})

	it("needs the every-Space read for an unfiltered page, and respects the read budget", async () => {
		const noEvery = setup({}, {every: false})
		await call(noEvery.shadow as never, FIXTURE.request.variables)
		expect(unsupported()).toEqual({no_space: 1})
		expect(noEvery.readBacklinks).not.toHaveBeenCalled()

		const budget = createReadBudget({perSecond: 0.001, burst: 1, now: () => 0})
		const s = setup({[geoSpace(SPACE)]: fixtureRefs()}, {budget})
		await call(s.shadow as never, FIXTURE.request.variables)
		await call(s.shadow as never, FIXTURE.request.variables)
		expect(results()).toMatchObject({match: 1})
		expect(unsupported()).toMatchObject({throttled: 1})
	})

	it("counts a failed read as failed, and an empty page against an empty answer as a match", async () => {
		const s = setup({})
		s.readBacklinks.mockRejectedValueOnce(new Error("boom"))
		await call(s.shadow as never, FIXTURE.request.variables)
		expect(results()).toEqual({failed: 1})
		await call(s.shadow as never, FIXTURE.request.variables, withRows([]))
		expect(results()).toEqual({failed: 1, match: 1})
	})

	it("leaves the operation unsupported when the flag is off", async () => {
		const s = setup({[geoSpace(SPACE)]: fixtureRefs()}, {flag: false})
		await call(s.shadow as never, FIXTURE.request.variables)
		expect(unsupported()).toEqual({selection: 1})
		expect(s.readBacklinks).not.toHaveBeenCalled()
	})

	describe("more loaded Spaces than one line can name (GEODB_EVERY_SPACE_CHUNKS)", () => {
		const filler = Array.from({length: 1024}, (_, i) => geoSpace((i + 1).toString(16).padStart(32, "0")))
		const many = [...filler, geoSpace(SPACE)]

		it("refuses while the flag is off", async () => {
			const s = setup({[geoSpace(SPACE)]: fixtureRefs()}, {spaces: many})
			await call(s.shadow as never, FIXTURE.request.variables)
			expect(unsupported()).toEqual({many_spaces: 1})
			expect(s.readBacklinks).not.toHaveBeenCalled()
		})

		it("reads line by line, names the Space of a lone last line, and matches", async () => {
			const s = setup({[geoSpace(SPACE)]: fixtureRefs()}, {spaces: many, chunks: true})
			await call(s.shadow as never, FIXTURE.request.variables)
			expect(results()).toEqual({match: 1})
			expect(s.readBacklinks.mock.calls.map((c) => c[0].geoSpaceIds.length)).toEqual([1024, 1])
		})

		it("issues the lines together, and an answer that lands out of order merges as the serial read does", async () => {
			const s = setup({[geoSpace(SPACE)]: fixtureRefs()}, {spaces: many, chunks: true})
			const answer = s.readBacklinks.getMockImplementation() as (a: never) => Promise<unknown>
			let inFlight = 0
			let widest = 0
			// The first line answers last: the merge must not depend on which line lands first.
			s.readBacklinks.mockImplementation((async (a: {geoSpaceIds: string[]}) => {
				inFlight += 1
				widest = Math.max(widest, inFlight)
				await new Promise((resolve) => setTimeout(resolve, a.geoSpaceIds.length > 1 ? 20 : 1))
				inFlight -= 1
				return answer(a as never)
			}) as never)
			await call(s.shadow as never, FIXTURE.request.variables)
			expect(widest).toBe(2)
			expect(results()).toEqual({match: 1})
			expect(s.readBacklinks.mock.calls.map((c) => c[0].geoSpaceIds.length)).toEqual([1024, 1])
		})

		it("takes every line from the budget first: a read that cannot have them all is throttled, none issued", async () => {
			const budget = createReadBudget({perSecond: 0, burst: 1})
			const s = setup({[geoSpace(SPACE)]: fixtureRefs()}, {spaces: many, chunks: true, budget})
			await call(s.shadow as never, FIXTURE.request.variables)
			expect(unsupported()).toEqual({throttled: 1})
			expect(s.readBacklinks).not.toHaveBeenCalled()
			// Nothing was taken in part: the one token is still there for a one-line read.
			expect(budget.tryAcquire(1)).toBe(true)
		})

		it("never has more lines in flight than the budget holds", async () => {
			const budget = createReadBudget({perSecond: 0, burst: 4})
			const s = setup({[geoSpace(SPACE)]: fixtureRefs()}, {spaces: many, chunks: true, budget})
			let inFlight = 0
			let widest = 0
			const answer = s.readBacklinks.getMockImplementation() as (a: never) => Promise<unknown>
			s.readBacklinks.mockImplementation((async (a: never) => {
				inFlight += 1
				widest = Math.max(widest, inFlight)
				await new Promise((resolve) => setTimeout(resolve, 5))
				inFlight -= 1
				return answer(a)
			}) as never)
			for (let i = 0; i < 4; i++) await call(s.shadow as never, FIXTURE.request.variables)
			// Two reads of two lines fit the four tokens; the other two are throttled.
			expect(widest).toBeLessThanOrEqual(4)
			expect(unsupported()).toEqual({throttled: 2})
		})

		it("flags a referrer geo-db holds in no line", async () => {
			const s = setup({}, {spaces: many, chunks: true})
			await call(s.shadow as never, FIXTURE.request.variables)
			expect(results()).toEqual({mismatch: 1})
		})
	})
})
