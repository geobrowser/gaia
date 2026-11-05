/**
 * Times backfill: give every entity geo-db already holds its createdAt and updatedAt, from Postgres.
 *
 * The feeder writes the two reserved properties (entityTimes.ts) only for edits it sees, so entities that predate
 * GEODB_ENTITY_TIMES have none. The source is gaia's `entities.created_at` / `updated_at`: it is what the frontend
 * is served today, it is complete (one row per entity), and one indexed read answers a whole batch. The version
 * tables would need a join per row and may not reach back past the topic's retention either.
 *
 * Each value is stamped into every Space where the entity has statements, so the minimum createdAt and the
 * maximum updatedAt across Spaces (how the api reads them) are the Postgres values.
 *
 * Safety, as for the other backfills (backfill.ts): run with the feeder scaled to 0. For each entity the cells are
 * read back from geo-db and only a property with no row is written, so nothing the live stream wrote is
 * overwritten. An entity geo-db holds no rows for is skipped (no phantom entities). The cursor is a per-Space
 * index into a list taken once, on the feeder's volume, separate from the main backfill's.
 *
 * Known residual: an entity first seen by the feeder after GEODB_ENTITY_TIMES went on already holds a "first seen"
 * createdAt, which this never replaces. It is later than Postgres's, which the shadow reports as inconclusive.
 */
import {existsSync, mkdirSync, readFileSync} from "node:fs"
import {type ApplyDeps, applyIntents} from "./apply.js"
import {CREATED_AT_PROPERTY, UPDATED_AT_PROPERTY, blockInstant, timeIntent} from "./entityTimes.js"
import type {GeoDb} from "./geodb.js"
import {type EntityReader, readWithRetry} from "./geodbPipe.js"
import {cid} from "./ids.js"
import {log} from "./log.js"
import type {Metrics} from "./metrics.js"
import {compactId} from "./pgAdapter.js"
import type {TimesSource} from "./pgSource.js"
import type {SpaceRecord, Store} from "./state.js"
import {writeFileDurable} from "./state.js"

export type TimesOptions = {
	/** Gaia space ids to visit; empty means every Space geo-db has. */
	spaces: string[]
	batch: number
	chunk: number
	pauseMs: number
	/** Stop after this many entities this run (0: no limit). */
	maxEntities: number
}

export type TimesDeps = {
	source: TimesSource
	reader: EntityReader
	db: Pick<GeoDb, "apply" | "waitReady">
	store: Store
	metrics: Metrics
	guard: () => Promise<void>
	sleep?: (ms: number) => Promise<void>
	signal?: AbortSignal
}

export type TimesStats = {
	spaces: number
	entities: number
	/** Entities with no row in geo-db for this Space (nothing to stamp). */
	absent: number
	/** Entities Postgres has no usable time for. */
	noTime: number
	createdWritten: number
	updatedWritten: number
	alreadyHeld: number
	deadLettered: number
	stoppedAtLimit: boolean
}

type Cursor = {done: Record<string, number>}

const defaultSleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

/** Postgres prints `created_at` as unix seconds text; anything else is not a time we can stamp. */
export function secondsFromText(text: string | null | undefined): number | null {
	if (text === null || text === undefined || !/^\d{1,12}$/.test(text.trim())) return null
	return Number(text.trim())
}

export async function runTimesBackfill(deps: TimesDeps, opts: TimesOptions): Promise<TimesStats> {
	const sleep = deps.sleep ?? defaultSleep
	const dir = `${deps.store.dir}/times-backfill`
	mkdirSync(dir, {recursive: true})
	const cursorPath = `${dir}/cursor.json`
	const cursor: Cursor = existsSync(cursorPath) ? {done: {}, ...JSON.parse(readFileSync(cursorPath, "utf8"))} : {done: {}}
	const stats: TimesStats = {
		spaces: 0,
		entities: 0,
		absent: 0,
		noTime: 0,
		createdWritten: 0,
		updatedWritten: 0,
		alreadyHeld: 0,
		deadLettered: 0,
		stoppedAtLimit: false,
	}
	const applyDeps: ApplyDeps = {
		db: {
			waitReady: (t?: number) => deps.db.waitReady(t),
			apply: async (zon: string) => {
				const r = await deps.db.apply(zon)
				if (opts.pauseMs > 0) await sleep(opts.pauseMs)
				return r
			},
		},
		store: deps.store,
		metrics: deps.metrics,
		beat: async () => {},
		sleep,
		signal: deps.signal,
	}
	const mapped = Object.entries(deps.store.spaces)
		.filter(([, r]) => r.geo)
		.map(([g]) => g)
		.sort()
	const plan = opts.spaces.length > 0 ? opts.spaces.map(compactId).filter((s) => deps.store.spaces[s]?.geo) : mapped
	log("info", "times backfill plan", {spaces: plan.length})

	for (const space of plan) {
		if (deps.signal?.aborted) throw new Error("shutting down")
		const geoSpace = deps.store.spaces[space] as SpaceRecord & {geo: string}
		const listPath = `${dir}/entities-${space}.txt`
		let ids: string[]
		if (existsSync(listPath)) ids = readFileSync(listPath, "utf8").split("\n").filter(Boolean)
		else {
			ids = await deps.source.listEntities(space)
			writeFileDurable(listPath, ids.map((i) => `${i}\n`).join(""))
		}
		const start = cursor.done[space] ?? 0
		if (start >= ids.length) continue
		stats.spaces++
		for (let from = start; from < ids.length; from += opts.batch) {
			if (deps.signal?.aborted) throw new Error("shutting down")
			if (opts.maxEntities > 0 && stats.entities >= opts.maxEntities) {
				stats.stoppedAtLimit = true
				return stats
			}
			await deps.guard()
			const to = Math.min(from + opts.batch, ids.length)
			const batchIds = ids.slice(from, to)
			const times = new Map((await deps.source.readTimes(batchIds)).map((r) => [compactId(r.id), r]))
			const intents: string[] = []
			for (const id of batchIds) {
				stats.entities++
				const t = times.get(compactId(id))
				const created = blockInstant(secondsFromText(t?.created_at))
				const updated = blockInstant(secondsFromText(t?.updated_at))
				if (created === null && updated === null) {
					stats.noTime++
					continue
				}
				const e = cid("entity", compactId(id))
				const held = await readWithRetry(deps.reader, geoSpace.geo, e, sleep)
				if (held.length === 0) {
					stats.absent++
					continue
				}
				const props = new Set(held.map((r) => r.prop))
				if (created !== null && !props.has(CREATED_AT_PROPERTY)) {
					intents.push(timeIntent(e, CREATED_AT_PROPERTY, created))
					stats.createdWritten++
				} else if (created !== null) stats.alreadyHeld++
				if (updated !== null && !props.has(UPDATED_AT_PROPERTY)) {
					intents.push(timeIntent(e, UPDATED_AT_PROPERTY, updated))
					stats.updatedWritten++
				} else if (updated !== null) stats.alreadyHeld++
			}
			if (intents.length > 0) {
				const r = await applyIntents(
					applyDeps,
					geoSpace,
					{space, partition: -1, offset: `times-backfill:${from}-${to}`},
					intents,
					opts.chunk,
					0,
					() => {},
				)
				stats.deadLettered += r.deadLettered
			}
			cursor.done[space] = to
			writeFileDurable(cursorPath, JSON.stringify(cursor))
		}
		log("info", "times backfill space done", {space, entities: ids.length, total: stats})
	}
	return stats
}
