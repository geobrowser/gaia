/**
 * Held-repair: write the values the pre-#1244 backfill skipped. Its `held.props` marked a property held when
 * geo-db had ANY card row on it, and a relation's type is a card row's property too, so a value sharing its property
 * with a relation type of the same entity (a text "" and relations typed by the same property) was skipped for good.
 *
 * This is a targeted pass, not a backfill rerun (the Space backfill's cursor skips its finished batches, and a rerun
 * needs the feeder at 0 for hours). It visits only the (Space, entity) pairs where a Postgres value shares its
 * property with a relation type of that entity, reads geo-db for each, and writes only the values geo-db lacks, through
 * the feeder's own translate + apply path and journals. Nothing geo-db holds is overwritten, relations are never
 * written, and a Space or entity geo-db does not have is left alone (counted): it founds nothing.
 *
 * Idempotent: a value written is a geo-db row, so a rerun finds its property held and writes nothing. The cursor
 * lives in its own directory (`<store>/held-repair`), so it never touches the Space backfill's. DRY_RUN reads and
 * counts but writes nothing at all (no geo-db write, no journal, no cursor), and logs counts only.
 */
import {existsSync, mkdirSync, readFileSync} from "node:fs"
import {type ApplyDeps, applyIntents} from "./apply.js"
import type {GeoDb} from "./geodb.js"
import {type EntityReader, readWithRetry} from "./geodbPipe.js"
import {cid} from "./ids.js"
import {log} from "./log.js"
import type {Metrics} from "./metrics.js"
import {compactId, entityOps, heldOf, type PgValueRow} from "./pgAdapter.js"
import type {HeldRepairSource} from "./pgSource.js"
import {Overlay, type Store, writeFileDurable} from "./state.js"
import {type GrcOp, translateOps} from "./translate.js"

export type HeldRepairOptions = {
	batch: number
	chunk: number
	pauseMs: number
	exclude: ReadonlySet<string>
	positions?: boolean
	/** Stop after this many entities this run (0: no limit). */
	maxEntities: number
	dryRun: boolean
}

export type HeldRepairDeps = {
	source: HeldRepairSource
	reader: EntityReader
	db: Pick<GeoDb, "apply" | "waitReady">
	store: Store
	metrics: Metrics
	guard: () => Promise<void>
	sleep?: (ms: number) => Promise<void>
	signal?: AbortSignal
}

export type HeldRepairStats = {
	dryRun: boolean
	candidatePairs: number
	spaces: number
	spacesUnmapped: number
	entities: number
	entitiesUnmapped: number
	entitiesAbsent: number
	entitiesToRepair: number
	valuesSharing: number
	valuesAlreadyHeld: number
	valuesToWrite: number
	valuesWritten: number
	intents: number
	deadLettered: number
	stoppedAtLimit: boolean
}

type Cursor = {done: Record<string, number>; inflight: {space: string; from: number; to: number} | null}

const defaultSleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

class HeldRepairState {
	readonly dir: string
	cursor: Cursor

	constructor(
		store: Store,
		private readonly persist: boolean,
	) {
		this.dir = `${store.dir}/held-repair`
		if (persist) mkdirSync(this.dir, {recursive: true})
		const path = `${this.dir}/cursor.json`
		this.cursor = existsSync(path) ? {done: {}, inflight: null, ...JSON.parse(readFileSync(path, "utf8"))} : {done: {}, inflight: null}
	}

	save(): void {
		if (this.persist) writeFileDurable(`${this.dir}/cursor.json`, JSON.stringify(this.cursor))
	}

	/** A Space's candidate list, taken once and kept, so the cursor indexes the same list on every rerun. */
	entities(space: string, fresh: () => string[]): string[] {
		const path = `${this.dir}/entities-${space}.txt`
		if (existsSync(path)) return readFileSync(path, "utf8").split("\n").filter(Boolean)
		const ids = fresh()
		if (this.persist) writeFileDurable(path, ids.map((i) => `${i}\n`).join(""))
		return ids
	}
}

export async function runHeldRepair(deps: HeldRepairDeps, opts: HeldRepairOptions): Promise<HeldRepairStats> {
	const sleep = deps.sleep ?? defaultSleep
	const state = new HeldRepairState(deps.store, !opts.dryRun)
	const stats: HeldRepairStats = {
		dryRun: opts.dryRun,
		candidatePairs: 0,
		spaces: 0,
		spacesUnmapped: 0,
		entities: 0,
		entitiesUnmapped: 0,
		entitiesAbsent: 0,
		entitiesToRepair: 0,
		valuesSharing: 0,
		valuesAlreadyHeld: 0,
		valuesToWrite: 0,
		valuesWritten: 0,
		intents: 0,
		deadLettered: 0,
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

	const pairs = await deps.source.listCandidates()
	stats.candidatePairs = pairs.length
	const bySpace = new Map<string, Set<string>>()
	for (const p of pairs) {
		const s = compactId(p.space)
		let set = bySpace.get(s)
		if (!set) bySpace.set(s, (set = new Set()))
		set.add(p.entity)
	}
	const plan = [...bySpace.keys()].sort()
	log("info", "held-repair plan", {dryRun: opts.dryRun, spaces: plan.length, pairs: pairs.length})

	for (const space of plan) {
		if (deps.signal?.aborted) throw new Error("shutting down")
		const ids = state.entities(space, () => [...(bySpace.get(space) as Set<string>)].sort())
		const record = deps.store.spaces[space]
		if (!record?.geo) {
			stats.spacesUnmapped++
			stats.entitiesUnmapped += ids.length
			continue
		}
		const geoSpace = record as typeof record & {geo: string}
		const start = state.cursor.done[space] ?? 0
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
			const resumed = state.cursor.inflight
			const suspect = resumed?.space === space && resumed.from === from
			state.cursor.inflight = {space, from, to}
			state.save()

			const {values, relations} = await deps.source.readBatch(space, ids.slice(from, to))
			const valuesBy = new Map<string, PgValueRow[]>()
			for (const v of values) {
				const k = compactId(v.entity_id)
				valuesBy.set(k, [...(valuesBy.get(k) ?? []), v])
			}
			const relsBy = new Map<string, typeof relations>()
			for (const r of relations) {
				const k = compactId(r.from_entity_id)
				relsBy.set(k, [...(relsBy.get(k) ?? []), r])
			}
			const overlay = new Overlay(deps.store, space)
			const ops: GrcOp[] = []
			for (const entity of ids.slice(from, to)) {
				const k = compactId(entity)
				const rels = relsBy.get(k) ?? []
				const types = new Set(rels.map((r) => compactId(r.type_id)))
				const sharing = (valuesBy.get(k) ?? []).filter((v) => types.has(compactId(v.property_id)) && !opts.exclude.has(compactId(v.property_id)))
				const e = cid("entity", k)
				stats.entities++
				stats.valuesSharing += sharing.length
				const known = deps.store.hasEntity(space, e)
				const geo = await readWithRetry(deps.reader, geoSpace.geo, e, sleep)
				if (!known && geo.length === 0) {
					// geo-db has nothing for it: the Space backfill creates it, with every value; this pass never creates.
					stats.entitiesAbsent++
					continue
				}
				if (!known) overlay.markEntity(e)
				const held = heldOf(geo, rels)
				// relations = []: this pass writes values only.
				const r = entityOps(k, sharing, [], held, opts.exclude)
				stats.valuesAlreadyHeld += sharing.length - r.values - r.skipped
				stats.valuesToWrite += r.values
				if (r.ops.length > 0) stats.entitiesToRepair++
				ops.push(...r.ops)
			}
			if (!opts.dryRun) {
				const {intents, stats: tstats} = translateOps(ops, overlay, {positions: opts.positions})
				const result = await applyIntents(
					applyDeps,
					geoSpace,
					{space, partition: -1, offset: `held-repair:${from}-${to}`},
					intents,
					opts.chunk,
					0,
					() => {},
				)
				overlay.commit()
				for (const [key, v] of Object.entries(tstats)) deps.metrics.opsDetail.inc({counter: key}, v)
				stats.valuesWritten += tstats["values-set"] ?? 0
				stats.intents += result.applied
				stats.deadLettered += result.deadLettered
			}
			state.cursor.done[space] = to
			state.cursor.inflight = null
			state.save()
		}
	}
	return stats
}
