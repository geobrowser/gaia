/**
 * The group B ranked reads' shadow (GEODB_RANKED_READS_SHADOW, off by default; plan 10.11 section 4, ZP-GROUPB-READS).
 *
 * For each ported read the twin in `rankedReads/trending.ts` (a pure function over row sets) is run on rows loaded from Postgres
 * and held to what the SQL function returns for the same call. It proves the twin on live data, shaped as the seam a geo-db
 * graph reader replaces: the `source.graph` loader is the only place rows come from, and swapping it for a geo-db read is what
 * turns this into the cutover check. Reads stay on Postgres; nothing here touches a request.
 *
 * Covered: `current_debate_agendas(NULL, 0)` and `(NULL, 7)` (which in turn exercise agenda_contents, entity_listings and the
 * helpers) and `topic_suggestions_for_spaces` for a sample of spaces.
 *
 * `gaia_api_ranked_read_shadow_total{read,result}`:
 *  - match: the twin returned exactly the rows (and order) the SQL did;
 *  - mismatch: it did not. A row set is read in several statements, so a write between them is a benign mismatch: read the
 *    rate, not one count;
 *  - failed: a read failed.
 *
 * Counts only: no entity id, space id or name is logged or labelled.
 */

import {sql} from "drizzle-orm"
import type {Database} from "../forYou/queries"
import {
	BLOCKS,
	CLAIMS,
	COLLECTION_ITEM,
	type CurrentDebateAgendaRow,
	DATE,
	DEBATES,
	type EntityRow,
	type Graph,
	NAME,
	TOPICS,
	Trending,
	TYPES,
} from "./rankedReads/trending"

export const READS = ["current_debate_agendas_0", "current_debate_agendas_7", "topic_suggestions_for_spaces"] as const
export type Read = (typeof READS)[number]
export const RESULTS = ["match", "mismatch", "failed"] as const
export type Result = (typeof RESULTS)[number]

export type RankedReadsShadowConfig = {enabled: boolean; intervalMs: number; spaces: number}

const wholeNumber = (raw: string | undefined, fallback: number, max: number) => {
	const n = Number(raw)
	return Number.isInteger(n) && n >= 1 ? Math.min(n, max) : fallback
}

export function readRankedReadsShadowConfig(env: Record<string, string | undefined>): RankedReadsShadowConfig {
	const flag = env.GEODB_RANKED_READS_SHADOW?.trim().toLowerCase()
	return {
		enabled: flag === "1" || flag === "on" || flag === "true",
		intervalMs: wholeNumber(env.GEODB_RANKED_READS_SHADOW_INTERVAL_MS, 300_000, 86_400_000),
		spaces: wholeNumber(env.GEODB_RANKED_READS_SHADOW_SPACES, 5, 50),
	}
}

/** A `current_debate_agendas` row as Postgres returns it, `listed_at` as epoch milliseconds. */
export type PgDebateRow = Omit<CurrentDebateAgendaRow, "listed_at"> & {listed_at: number}

/** Where a pass gets its rows and the Postgres answers. The default reads Postgres; tests inject. */
export type RankedReadsSource = {
	today(): Promise<string>
	/** The rows every ported read needs, loaded once per pass. */
	graph(today: string, spaceSample: string[]): Promise<Graph>
	sampleSpaces(limit: number): Promise<string[]>
	pgCurrentDebateAgendas(earlier: number): Promise<PgDebateRow[]>
	pgTopicSuggestions(spaces: string[]): Promise<EntityRow[]>
}

const key = (r: object): string => JSON.stringify(Object.entries(r).sort(([a], [b]) => (a < b ? -1 : 1)))

/** Whether two ordered row lists are the same. Pure. */
export function sameRows(twin: object[], pg: object[]): boolean {
	return twin.length === pg.length && twin.every((r, i) => key(r) === key(pg[i]!))
}

/** A twin row with `listed_at` as epoch milliseconds, the form the Postgres reader returns it in. */
const withEpoch = (r: CurrentDebateAgendaRow): PgDebateRow => ({...r, listed_at: Date.parse(r.listed_at)})

let active = false
const results = new Map<string, number>()
const bump = (read: Read, result: Result): void =>
	void results.set(`${read}|${result}`, (results.get(`${read}|${result}`) ?? 0) + 1)

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once active every series exists at zero. */
export function renderRankedReadsShadowMetrics(): string {
	if (!active) return ""
	const lines = [
		"# HELP gaia_api_ranked_read_shadow_total Group B ranked reads: the pure twin run on Postgres rows against the SQL function, by read and result (match, mismatch, failed). A write between the shadow's reads is a benign mismatch.",
		"# TYPE gaia_api_ranked_read_shadow_total counter",
	]
	for (const read of READS)
		for (const result of RESULTS)
			lines.push(
				`gaia_api_ranked_read_shadow_total{read="${read}",result="${result}"} ${results.get(`${read}|${result}`) ?? 0}`,
			)
	return lines.join("\n")
}

export function __resetRankedReadsShadowMetricsForTests(): void {
	active = false
	results.clear()
}

export type RankedReadsShadow = {runOnce(): Promise<void>; start(): () => void}

export function createRankedReadsShadow(deps: {
	config: RankedReadsShadowConfig
	source: RankedReadsSource
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
}): RankedReadsShadow {
	const {config, source, log} = deps
	active = true
	let running = false
	let logged = 0

	const record = (read: Read, ok: boolean) => {
		bump(read, ok ? "match" : "mismatch")
		if (!ok && logged < 20) {
			logged++
			log.warn("ranked reads shadow disagreement", {read})
		}
	}
	const fail = (read: Read, error: unknown) => {
		bump(read, "failed")
		log.warn("ranked reads shadow failed", {read, error: String(error)})
	}

	async function runOnce(): Promise<void> {
		if (running) return
		running = true
		try {
			let today: string
			let spaces: string[]
			let twin: Trending
			let graph: Graph
			try {
				today = await source.today()
				spaces = await source.sampleSpaces(config.spaces)
				graph = await source.graph(today, spaces)
				twin = new Trending(graph, today)
			} catch (error) {
				for (const read of READS) fail(read, error)
				return
			}
			for (const earlier of [0, 7] as const) {
				const read = `current_debate_agendas_${earlier}` as Read
				try {
					const pg = await source.pgCurrentDebateAgendas(earlier)
					record(read, sameRows(twin.currentDebateAgendas(null, earlier).map(withEpoch), pg))
				} catch (error) {
					fail(read, error)
				}
			}
			try {
				const pg = await source.pgTopicSuggestions(spaces)
				record("topic_suggestions_for_spaces", sameRows(twin.topicSuggestionsForSpaces(spaces), pg))
			} catch (error) {
				fail("topic_suggestions_for_spaces", error)
			}
		} finally {
			running = false
		}
	}

	return {
		runOnce,
		start() {
			const timer = setInterval(() => void runOnce(), config.intervalMs)
			timer.unref?.()
			return () => clearInterval(timer)
		},
	}
}

// ---- the Postgres source --------------------------------------------------------------------------------------------------

const rowsOf = (result: unknown) =>
	((result as {rows?: Record<string, unknown>[]}).rows ?? []) as Record<string, unknown>[]
const CHUNK = 500

function idList(ids: string[]) {
	return sql.join(
		ids.map((id) => sql`${id}::uuid`),
		sql`, `,
	)
}

/** Run `read` over `ids` in chunks, concatenating. */
async function chunked<T>(ids: string[], read: (chunk: string[]) => Promise<T[]>): Promise<T[]> {
	const out: T[] = []
	const unique = [...new Set(ids)]
	for (let i = 0; i < unique.length; i += CHUNK) out.push(...(await read(unique.slice(i, i + CHUNK))))
	return out
}

const RELATION_COLUMNS = sql.raw(
	'r.id::text AS id, r.type_id::text AS type_id, r.from_entity_id::text AS from_entity_id, r.to_entity_id::text AS to_entity_id, r."position" AS position, r.space_id::text AS space_id',
)
const VALUE_COLUMNS = sql.raw(
	'v.id::text AS id, v.property_id::text AS property_id, v.entity_id::text AS entity_id, v.space_id::text AS space_id, v."text" AS text, to_char(v.datetime_utc AT TIME ZONE \'UTC\', \'YYYY-MM-DD"T"HH24:MI:SS"Z"\') AS datetime_utc, v.datetime AS datetime, v."date" AS date',
)

type Rel = Graph["relations"][number]
type Val = Graph["values"][number]

const asRel = (r: Record<string, unknown>): Rel => ({
	id: String(r.id),
	type_id: String(r.type_id),
	from_entity_id: String(r.from_entity_id),
	to_entity_id: String(r.to_entity_id),
	position: r.position === null ? null : String(r.position),
	space_id: String(r.space_id),
})
const asVal = (r: Record<string, unknown>): Val => ({
	id: String(r.id),
	property_id: String(r.property_id),
	entity_id: String(r.entity_id),
	space_id: String(r.space_id),
	text: r.text === null ? null : String(r.text),
	datetime_utc: r.datetime_utc === null ? null : String(r.datetime_utc),
	datetime: r.datetime === null ? null : String(r.datetime),
	date: r.date === null ? null : String(r.date),
})

/** How far back the shadow reads agendas: the widest `earlier` window it asks for, with the current week. */
const LOOKBACK_DAYS = 40

export function createPostgresRankedReadsSource(db: Database): RankedReadsSource {
	const relationsFrom = (ids: string[], types?: string[]) =>
		chunked(ids, async (c) =>
			rowsOf(
				await db.execute(
					types === undefined
						? sql`SELECT ${RELATION_COLUMNS} FROM public.relations r WHERE r.from_entity_id IN (${idList(c)})`
						: sql`SELECT ${RELATION_COLUMNS} FROM public.relations r WHERE r.from_entity_id IN (${idList(c)}) AND r.type_id IN (${idList(types)})`,
				),
			).map(asRel),
		)
	const valuesOf = (ids: string[], property: string) =>
		chunked(ids, async (c) =>
			rowsOf(
				await db.execute(
					sql`SELECT ${VALUE_COLUMNS} FROM public."values" v WHERE v.entity_id IN (${idList(c)}) AND v.property_id = ${property}::uuid`,
				),
			).map(asVal),
		)

	return {
		async today() {
			return String(rowsOf(await db.execute(sql`SELECT trending.today()::text AS today`))[0]?.today)
		},

		async sampleSpaces(limit) {
			return rowsOf(
				await db.execute(
					sql`SELECT s.space_id::text AS id FROM (SELECT DISTINCT space_id FROM public.space_topic_suggestions) s ORDER BY random() LIMIT ${limit}`,
				),
			).map((r) => String(r.id))
		},

		async graph(today, spaceSample) {
			const spaces = rowsOf(
				await db.execute(sql`SELECT id::text AS id, type, topic_id::text AS topic_id FROM public.spaces`),
			).map((r) => ({
				id: String(r.id),
				type: String(r.type),
				topic_id: r.topic_id === null ? null : String(r.topic_id),
			}))
			const agendaIds = rowsOf(
				await db.execute(
					sql`SELECT DISTINCT agenda_id::text AS id FROM trending.current_agendas() WHERE listed_on >= ${today}::date - ${LOOKBACK_DAYS}`,
				),
			).map((r) => String(r.id))
			const relations = new Map<string, Rel>()
			const values = new Map<string, Val>()
			const addRels = (rs: Rel[]) => rs.forEach((r) => relations.set(r.id, r))
			const addVals = (vs: Val[]) => vs.forEach((v) => values.set(v.id, v))

			const agendaRels = await relationsFrom(agendaIds)
			addRels(agendaRels)
			addVals(await valuesOf(agendaIds, DATE))
			const blockIds = agendaRels.filter((r) => r.type_id === BLOCKS).map((r) => r.to_entity_id)
			const blockRels = await relationsFrom(blockIds, [COLLECTION_ITEM, TOPICS])
			addRels(blockRels)
			addVals(await valuesOf(blockIds, NAME))

			// Everything listed: block items and the Claims / Debates targets.
			const listed = new Set<string>([
				...blockRels.filter((r) => r.type_id === COLLECTION_ITEM).map((r) => r.to_entity_id),
				...agendaRels.filter((r) => r.type_id === CLAIMS || r.type_id === DEBATES).map((r) => r.to_entity_id),
			])
			addRels(await relationsFrom([...listed], [TYPES, TOPICS]))
			// Claims and debates the listed debates link, and the debates that link the listed claims (entity_listings).
			const linked = [
				...(await relationsFrom([...listed], [CLAIMS])),
				...(await chunked([...listed], async (c) =>
					rowsOf(
						await db.execute(
							sql`SELECT ${RELATION_COLUMNS} FROM public.relations r WHERE r.to_entity_id IN (${idList(c)}) AND r.type_id = ${CLAIMS}::uuid`,
						),
					).map(asRel),
				)),
			]
			addRels(linked)
			const everyone = new Set<string>(listed)
			for (const r of linked) {
				everyone.add(r.from_entity_id)
				everyone.add(r.to_entity_id)
			}
			addRels(await relationsFrom([...everyone], [TYPES]))

			const listings = await chunked([...everyone], async (c) =>
				rowsOf(
					await db.execute(
						sql`SELECT source, space_id::text AS space_id, agenda_id::text AS agenda_id, entity_id::text AS entity_id, listed_on::text AS listed_on FROM trending.agenda_listings WHERE entity_id IN (${idList(c)})`,
					),
				).map((r) => ({
					source: String(r.source),
					space_id: String(r.space_id),
					agenda_id: String(r.agenda_id),
					entity_id: String(r.entity_id),
					listed_on: String(r.listed_on),
				})),
			)

			const suggestions = await chunked(spaceSample, async (c) =>
				rowsOf(
					await db.execute(
						sql`SELECT space_id::text AS space_id, topic_id::text AS topic_id, name_key, links, rank FROM public.space_topic_suggestions WHERE space_id IN (${idList(c)})`,
					),
				).map((r) => ({
					space_id: String(r.space_id),
					topic_id: String(r.topic_id),
					name_key: String(r.name_key),
					links: Number(r.links),
					rank: Number(r.rank),
				})),
			)
			const topicIds = [
				...new Set([
					...suggestions.map((s) => s.topic_id),
					...spaces
						.filter((s) => spaceSample.includes(s.id))
						.flatMap((s) => (s.topic_id === null ? [] : [s.topic_id])),
				]),
			]
			addVals(await valuesOf(topicIds, NAME))

			const entityIds = [...agendaIds, ...topicIds]
			const entities = await chunked(entityIds, async (c) =>
				rowsOf(
					await db.execute(
						sql`SELECT id::text AS id, created_at FROM public.entities WHERE id IN (${idList(c)})`,
					),
				).map((r) => ({
					id: String(r.id),
					created_at: String(r.created_at),
				})),
			)
			return {
				entities,
				spaces,
				relations: [...relations.values()],
				values: [...values.values()],
				listings,
				suggestions,
			}
		},

		async pgCurrentDebateAgendas(earlier) {
			return rowsOf(
				await db.execute(
					sql`SELECT space_id::text AS space_id, agenda_id::text AS agenda_id, listed_on::text AS listed_on, (extract(epoch FROM listed_at) * 1000)::bigint::text AS listed_at, is_current, source, section_id::text AS section_id, section_name, section_topic_id::text AS section_topic_id, section_position, entity_id::text AS entity_id, item_position, last_listed_on::text AS last_listed_on FROM public.current_debate_agendas(NULL, ${earlier})`,
				),
			).map((r) => ({
				space_id: String(r.space_id),
				agenda_id: String(r.agenda_id),
				listed_on: String(r.listed_on),
				listed_at: Number(r.listed_at),
				is_current: r.is_current === true,
				source: String(r.source),
				section_id: r.section_id === null ? null : String(r.section_id),
				section_name: r.section_name === null ? null : String(r.section_name),
				section_topic_id: r.section_topic_id === null ? null : String(r.section_topic_id),
				section_position: r.section_position === null ? null : Number(r.section_position),
				entity_id: String(r.entity_id),
				item_position: r.item_position === null ? null : Number(r.item_position),
				last_listed_on: r.last_listed_on === null ? null : String(r.last_listed_on),
			}))
		},

		async pgTopicSuggestions(spaces) {
			if (spaces.length === 0) return []
			return rowsOf(
				await db.execute(
					sql`SELECT id::text AS id, created_at FROM public.topic_suggestions_for_spaces(ARRAY[${idList(spaces)}]::uuid[])`,
				),
			).map((r) => ({id: String(r.id), created_at: String(r.created_at)}))
		},
	}
}
