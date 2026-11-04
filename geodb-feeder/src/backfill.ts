/**
 * Backfill: load into geo-db the current state of every entity the replica is missing, from Postgres.
 *
 * The edit topic's retention has truncated history, so the feeder only knows entities edited inside the
 * window. This reads the kg-indexer's tables and writes what geo-db lacks through the feeder's own
 * translator (`translateOps`), apply path (`applyIntents`), journals (`Overlay`) and Space founding
 * (`ensureSpace`), so every row it writes is the row the feeder would have written for the same edit.
 *
 * Ordering (see ordering.ts for the precondition, checked before and during the run): the feeder is stopped at
 * offset O and Postgres already reflects every edit through O. For each entity the backfill writes only
 * what geo-db does not hold:
 * - an entity the journals do not know gets all its values and relations, and is journaled;
 * - an entity they know is read back from geo-db, and only properties with no row and relations whose
 *   statement id is absent are written.
 * Nothing geo-db holds is overwritten, so no live edit through O is undone. Edits after O are replayed by the
 * feeder once it restarts, in order, on top of what the backfill wrote: a `set` replaces the backfilled cell,
 * an `unset`, `deleteEntity` or `deleteRelation` applies because the journals now know the entity or
 * statement, and a create of something backfilled is skipped as known. Postgres being read later than O
 * (the indexer keeps going) is harmless: the cell then holds a value from after O, and the replay of every
 * later edit to that cell lands on top of it in order.
 *
 * Idempotence: progress is a per-Space cursor on the feeder's volume, advanced after each batch is applied
 * and journaled. A batch interrupted mid-apply is recorded as in flight; on the rerun its entities are all
 * read back from geo-db, so nothing they already got is written twice.
 */
import {existsSync, mkdirSync, readFileSync} from "node:fs"
import {type ApplyDeps, applyIntents} from "./apply.js"
import type {GeoDb} from "./geodb.js"
import {type EntityReader, readWithRetry} from "./geodbPipe.js"
import {cid} from "./ids.js"
import {log} from "./log.js"
import type {Metrics} from "./metrics.js"
import {DERIVED_PROPERTIES, compactId, entityOps, type Held, type PgRelationRow, type PgValueRow} from "./pgAdapter.js"
import type {BackfillSource} from "./pgSource.js"
import {ensureSpace, SpaceCapError, type SpaceDeps} from "./spaces.js"
import {Overlay, type SpaceRecord, type Store, writeFileDurable} from "./state.js"
import {type GrcOp, translateOps} from "./translate.js"

export type Scope = {kind: "mapped"} | {kind: "all"} | {kind: "list"; spaces: string[]}

export function parseScope(text: string | undefined): Scope {
	const t = (text ?? "").trim()
	if (t === "mapped") return {kind: "mapped"}
	if (t === "all") return {kind: "all"}
	const spaces = t
		.split(",")
		.map((s) => compactId(s.trim()))
		.filter(Boolean)
	if (spaces.length === 0 || spaces.some((s) => !/^[0-9a-f]{32}$/.test(s))) {
		throw new Error(`BACKFILL_SPACES must be "mapped", "all" or a comma list of space ids, not ${JSON.stringify(t)}`)
	}
	return {kind: "list", spaces}
}

/** BACKFILL_EXCLUDE_PROPERTIES: unset means the derived properties, "none" means none. */
export function excludeFrom(text: string | undefined): Set<string> {
	if (text === undefined || text.trim() === "") return new Set(DERIVED_PROPERTIES)
	if (text.trim() === "none") return new Set()
	return new Set(
		text
			.split(",")
			.map((s) => compactId(s.trim()))
			.filter(Boolean),
	)
}

export type BackfillOptions = {
	scope: Scope
	/** Never found a Space that would take geo-db past this many. */
	spaceCap: number
	/** Entities read from Postgres and applied together. */
	batch: number
	/** Intents per geodb-client apply (the feeder's CHUNK). */
	chunk: number
	/** Pause after every apply call, so serve keeps time for reads and for the feeder once it is back. */
	pauseMs: number
	exclude: ReadonlySet<string>
	/** Write relation positions (GEODB_RELATION_POSITIONS). */
	positions?: boolean
	/** Stop after this many entities this run (0: no limit). For a measured first run. */
	maxEntities: number
}

export type BackfillDeps = {
	source: BackfillSource
	reader: EntityReader
	db: Pick<GeoDb, "apply" | "waitReady" | "found">
	store: Store
	metrics: Metrics
	sealer: () => Promise<string>
	publish: (text: string) => void
	/** Throws when it is no longer safe to go on (the feeder came back). Called between batches. */
	guard: () => Promise<void>
	sleep?: (ms: number) => Promise<void>
	signal?: AbortSignal
}

type Cursor = {done: Record<string, number>; inflight: {space: string; from: number; to: number} | null}

export type BackfillStats = {
	spaces: number
	spacesFounded: number
	spacesSkippedCap: string[]
	entities: number
	unknownEntities: number
	knownEntities: number
	knownEntitiesFilled: number
	values: number
	relations: number
	intents: number
	deadLettered: number
	skippedRows: number
	excludedRows: number
	stoppedAtLimit: boolean
}

const defaultSleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

class BackfillState {
	readonly dir: string
	cursor: Cursor

	constructor(store: Store) {
		this.dir = `${store.dir}/backfill`
		mkdirSync(this.dir, {recursive: true})
		const path = `${this.dir}/cursor.json`
		this.cursor = existsSync(path) ? {done: {}, inflight: null, ...JSON.parse(readFileSync(path, "utf8"))} : {done: {}, inflight: null}
	}

	save(): void {
		writeFileDurable(`${this.dir}/cursor.json`, JSON.stringify(this.cursor))
	}

	/** A Space's entity list, taken once and kept, so the cursor indexes the same list on every rerun. */
	async entities(source: BackfillSource, space: string): Promise<string[]> {
		const path = `${this.dir}/entities-${space}.txt`
		if (existsSync(path)) return readFileSync(path, "utf8").split("\n").filter(Boolean)
		const ids = await source.listEntities(space)
		writeFileDurable(path, ids.map((i) => `${i}\n`).join(""))
		return ids
	}
}

/** The Spaces to visit, in order: the ones geo-db already has, then new ones largest first. */
export async function planSpaces(scope: Scope, store: Store, source: BackfillSource): Promise<string[]> {
	const mapped = Object.entries(store.spaces)
		.filter(([, r]) => r.geo)
		.map(([g]) => g)
	if (scope.kind === "list") return [...new Set(scope.spaces)]
	if (scope.kind === "mapped") return mapped.sort()
	const sizes = await source.spaceSizes()
	const size = (s: string) => sizes.get(s) ?? 0
	const mappedSet = new Set(mapped)
	const fresh = [...sizes.keys()].filter((s) => !mappedSet.has(s) && size(s) > 0)
	return [...mapped.sort((a, b) => size(b) - size(a)), ...fresh.sort((a, b) => size(b) - size(a) || a.localeCompare(b))]
}

export async function runBackfill(deps: BackfillDeps, opts: BackfillOptions): Promise<BackfillStats> {
	const sleep = deps.sleep ?? defaultSleep
	const state = new BackfillState(deps.store)
	const stats: BackfillStats = {
		spaces: 0,
		spacesFounded: 0,
		spacesSkippedCap: [],
		entities: 0,
		unknownEntities: 0,
		knownEntities: 0,
		knownEntitiesFilled: 0,
		values: 0,
		relations: 0,
		intents: 0,
		deadLettered: 0,
		skippedRows: 0,
		excludedRows: 0,
		stoppedAtLimit: false,
	}
	const pacedDb: ApplyDeps["db"] = {
		waitReady: (t?: number) => deps.db.waitReady(t),
		apply: async (zon: string) => {
			const r = await deps.db.apply(zon)
			if (opts.pauseMs > 0) await sleep(opts.pauseMs)
			return r
		},
	}
	const applyDeps: ApplyDeps = {db: pacedDb, store: deps.store, metrics: deps.metrics, beat: async () => {}, sleep, signal: deps.signal}
	const spaceDeps: SpaceDeps = {db: deps.db, store: deps.store, metrics: deps.metrics, sealer: deps.sealer, publish: deps.publish}
	const founded = () => Object.values(deps.store.spaces).filter((s) => s.geo).length

	const plan = await planSpaces(opts.scope, deps.store, deps.source)
	log("info", "backfill plan", {scope: opts.scope.kind, spaces: plan.length, founded: founded(), cap: opts.spaceCap})

	for (const space of plan) {
		if (deps.signal?.aborted) throw new Error("shutting down")
		const ids = await state.entities(deps.source, space)
		const start = state.cursor.done[space] ?? 0
		if (start >= ids.length) continue
		let record = deps.store.spaces[space]
		if (!record?.geo) {
			if (founded() >= opts.spaceCap) {
				stats.spacesSkippedCap.push(space)
				log("warn", "space not founded: at the space cap", {space, entities: ids.length, cap: opts.spaceCap})
				continue
			}
			try {
				record = await ensureSpace(spaceDeps, space)
			} catch (e) {
				if (!(e instanceof SpaceCapError)) throw e
				stats.spacesSkippedCap.push(space)
				continue
			}
			// A pipe opened before the founding holds no grant over the new Space.
			deps.reader.reset()
			stats.spacesFounded++
		}
		const geoSpace = record as SpaceRecord & {geo: string}
		stats.spaces++
		log("info", "backfill space", {space, geoSpace: geoSpace.geo, entities: ids.length, from: start})

		for (let from = start; from < ids.length; from += opts.batch) {
			if (deps.signal?.aborted) throw new Error("shutting down")
			if (opts.maxEntities > 0 && stats.entities >= opts.maxEntities) {
				stats.stoppedAtLimit = true
				return stats
			}
			await deps.guard()
			const to = Math.min(from + opts.batch, ids.length)
			const resumed = state.cursor.inflight
			const suspect = resumed?.space === space && resumed.from === from
			state.cursor.inflight = {space, from, to}
			state.save()

			const batchIds = ids.slice(from, to)
			const {values, relations} = await deps.source.readBatch(space, batchIds)
			const byEntity = groupRows(values, relations)
			const overlay = new Overlay(deps.store, space)
			const ops: GrcOp[] = []
			for (const entity of batchIds) {
				const rows = byEntity.get(compactId(entity)) ?? {values: [], relations: []}
				const e = cid("entity", compactId(entity))
				const known = deps.store.hasEntity(space, e)
				let held: Held | null = null
				if (known || suspect) {
					const geo = await readWithRetry(deps.reader, geoSpace.geo, e, sleep)
					if (known || geo.length > 0) {
						held = {props: new Set(geo.map((r) => r.prop)), statements: new Set(geo.map((r) => r.statement))}
						// Applied by the interrupted run but not journaled yet: known from here on.
						if (!known) overlay.markEntity(e)
					}
				}
				const r = entityOps(entity, rows.values, rows.relations, held, opts.exclude)
				stats.entities++
				if (held) {
					stats.knownEntities++
					if (r.ops.length > 0) stats.knownEntitiesFilled++
				} else stats.unknownEntities++
				stats.skippedRows += r.skipped
				stats.excludedRows += r.excluded
				ops.push(...r.ops)
			}
			const {intents, stats: tstats} = translateOps(ops, overlay, {positions: opts.positions})
			const result = await applyIntents(
				applyDeps,
				geoSpace,
				{space, partition: -1, offset: `backfill:${from}-${to}`},
				intents,
				opts.chunk,
				0,
				() => {},
			)
			overlay.commit()
			state.cursor.done[space] = to
			state.cursor.inflight = null
			state.save()
			for (const [k, v] of Object.entries(tstats)) deps.metrics.opsDetail.inc({counter: k}, v)
			stats.values += tstats["values-set"] ?? 0
			stats.relations += tstats.createRelation ?? 0
			stats.intents += result.applied
			stats.deadLettered += result.deadLettered
		}
		log("info", "backfill space done", {space, entities: ids.length, total: {...stats, spacesSkippedCap: stats.spacesSkippedCap.length}})
	}
	return stats
}

function groupRows(values: PgValueRow[], relations: PgRelationRow[]) {
	const by = new Map<string, {values: PgValueRow[]; relations: PgRelationRow[]}>()
	const at = (id: string) => {
		const k = compactId(id)
		let g = by.get(k)
		if (!g) {
			g = {values: [], relations: []}
			by.set(k, g)
		}
		return g
	}
	for (const v of values) at(v.entity_id).values.push(v)
	for (const r of relations) at(r.from_entity_id).relations.push(r)
	return by
}
