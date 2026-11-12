/**
 * Relation-seed: give the feeder a `RelationRecord` for every relation that existed before GEODB_RELATION_POSITIONS
 * was on. The feeder records a relation's cell (entity, property) and open statement id only when it creates the
 * relation itself, so a move of an older relation is dropped as `relation-move-cell-unknown`. This pass writes those
 * records.
 *
 * Source of truth: geo-db first, Postgres for the list. For each (Space, entity) with an outgoing relation in
 * Postgres it reads the entity's card with claims from geo-db and keeps a relation only when geo-db holds its
 * statement open under the id the feeder derives from the gaia relation id. That read is what answers "which
 * statement is open for this relation": the feeder moved no position before the flag (the moves were dropped), so an
 * older relation's statement is still under its original id, and the read confirms it. The facet claims on that
 * statement are read in the same pass, because a move writes them again on the replacement statement.
 *
 * It writes nothing to geo-db. Its only write is the feeder's `known-relations.log` (and `known-statements.log` for a
 * statement geo-db holds that the journal lacks, which `updateRelation` also needs). It never replaces a record the
 * feeder already holds: those are current, including after a move.
 *
 * Idempotent: a rerun finds the records and counts them as already recorded. The cursor lives in its own directory
 * (`<store>/relation-seed`). DRY_RUN reads and counts but writes no journal line and no cursor.
 *
 * Not covered: a position move the feeder already dropped before this ran stays dropped. Postgres holds the new
 * position, geo-db the old one, and the card read does not return positions, so this pass cannot tell which relations
 * differ. See the PR for the read that would let a later pass repair them.
 */
import {existsSync, mkdirSync, readFileSync} from "node:fs"
import type {RelationReader, GeoDbClaimRow} from "./geodbPipe.js"
import {cid} from "./ids.js"
import {log} from "./log.js"
import {compactId, type PgRelationRow} from "./pgAdapter.js"
import type {RelationSeedSource} from "./pgSource.js"
import {type Store, writeFileDurable} from "./state.js"
import {RELATION_FACETS, RELATION_FACET_PROPERTY, type RelationRecord} from "./translate.js"

export type RelationSeedOptions = {
	/** Entities per Postgres read and per journal write. */
	batch: number
	pauseMs: number
	/** Stop after this many entities this run (0: no limit). */
	maxEntities: number
	dryRun: boolean
}

export type RelationSeedDeps = {
	source: RelationSeedSource
	reader: RelationReader
	store: Store
	guard: () => Promise<void>
	sleep?: (ms: number) => Promise<void>
	signal?: AbortSignal
	/** Clock for the progress lines (tests). */
	now?: () => number
	/** Receives each progress line's fields (tests); default logs them as `relation-seed progress`. */
	onProgress?: (fields: Record<string, unknown>) => void
}

/** A progress line is logged when a Space finishes and at most this often inside a long one. */
const PROGRESS_EVERY_MS = 30_000

export type RelationSeedStats = {
	dryRun: boolean
	spaces: number
	spacesUnmapped: number
	entities: number
	relations: number
	/** Relations the feeder already holds a record for (its own, or an earlier seed): left as they are. */
	alreadyRecorded: number
	/** Relations whose statement geo-db does not hold open under the derived id (not applied yet, or absent). */
	notOpen: number
	/** Relations a record was (or, in a dry run, would be) written for. */
	toSeed: number
	seeded: number
	withFacets: number
	/** A statement geo-db holds that the statements journal lacked, added with the record. */
	statementsMarked: number
	/** A facet claim geo-db holds with a value the feeder does not carry (a malformed claim); the facet is left out. */
	facetsUnreadable: number
	stoppedAtLimit: boolean
}

type Cursor = {done: Record<string, number>; inflight: {space: string; from: number; to: number} | null}

const defaultSleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

class SeedState {
	readonly dir: string
	cursor: Cursor

	constructor(
		store: Store,
		private readonly persist: boolean,
	) {
		this.dir = `${store.dir}/relation-seed`
		if (persist) mkdirSync(this.dir, {recursive: true})
		const path = `${this.dir}/cursor.json`
		this.cursor = existsSync(path) ? {done: {}, inflight: null, ...JSON.parse(readFileSync(path, "utf8"))} : {done: {}, inflight: null}
	}

	save(): void {
		if (this.persist) writeFileDurable(`${this.dir}/cursor.json`, JSON.stringify(this.cursor))
	}

	/** A Space's entity list, taken once and kept, so the cursor indexes the same list on every rerun. */
	async entities(space: string, fresh: () => Promise<string[]>): Promise<string[]> {
		const path = `${this.dir}/entities-${space}.txt`
		if (existsSync(path)) return readFileSync(path, "utf8").split("\n").filter(Boolean)
		const ids = await fresh()
		if (this.persist) writeFileDurable(path, ids.map((i) => `${i}\n`).join(""))
		return ids
	}
}

const FACET_OF_PROPERTY = new Map(RELATION_FACETS.map(({field}) => [RELATION_FACET_PROPERTY[field], field as string]))
FACET_OF_PROPERTY.set(RELATION_FACET_PROPERTY.verified, "verified")

/** The facets the claims on `statement` hold, in the form `RelationRecord.facets` keeps (hex ids, "true" for verified). */
function facetsOf(rows: GeoDbClaimRow[], statement: string, onUnreadable: () => void): Record<string, string> {
	const held: Record<string, string> = {}
	for (const r of rows) {
		if (r.statement !== statement || !r.claimProp) continue
		const field = FACET_OF_PROPERTY.get(r.claimProp)
		if (!field) continue
		const raw = (r.claimRaw ?? "").toLowerCase()
		if (field === "verified") {
			if (raw === "true") held.verified = "true"
			else onUnreadable()
		} else if (/^[0-9a-f]{32}$/.test(raw)) held[field] = raw
		else onUnreadable()
	}
	return held
}

async function readClaims(reader: RelationReader, geoSpace: string, entity: string, sleep: (ms: number) => Promise<void>, attempts = 4) {
	let last: unknown
	for (let i = 0; i < attempts; i++) {
		try {
			return await reader.entityClaimRows(geoSpace, entity)
		} catch (e) {
			last = e
			reader.reset()
			await sleep(1000 * 2 ** i)
		}
	}
	throw last
}

export async function runRelationSeed(deps: RelationSeedDeps, opts: RelationSeedOptions): Promise<RelationSeedStats> {
	const sleep = deps.sleep ?? defaultSleep
	const state = new SeedState(deps.store, !opts.dryRun)
	const stats: RelationSeedStats = {
		dryRun: opts.dryRun,
		spaces: 0,
		spacesUnmapped: 0,
		entities: 0,
		relations: 0,
		alreadyRecorded: 0,
		notOpen: 0,
		toSeed: 0,
		seeded: 0,
		withFacets: 0,
		statementsMarked: 0,
		facetsUnreadable: 0,
		stoppedAtLimit: false,
	}
	const plan = Object.keys(deps.store.spaces).sort()
	log("info", "relation-seed plan", {dryRun: opts.dryRun, spaces: plan.length})

	// Progress, so an operator can estimate the finish: Spaces visited out of the plan (an unmapped or already finished
	// Space counts as visited), counts so far, and the rate of this run (entities per second since it started).
	const now = deps.now ?? Date.now
	const startedAt = now()
	let lastProgress = startedAt
	let visited = 0
	const progress = (space: string, spaceDone: boolean) => {
		const t = now()
		const seconds = Math.max((t - startedAt) / 1000, 0.001)
		lastProgress = t
		const fields = {
			dryRun: opts.dryRun,
			spacesDone: visited,
			spacesTotal: plan.length,
			spaceDone,
			entities: stats.entities,
			relations: stats.relations,
			toSeed: stats.toSeed,
			seeded: stats.seeded,
			notOpen: stats.notOpen,
			elapsedSeconds: Math.round(seconds),
			entitiesPerSecond: Math.round((stats.entities / seconds) * 10) / 10,
		}
		if (deps.onProgress) deps.onProgress({space, ...fields})
		else log("info", "relation-seed progress", fields)
	}

	for (const space of plan) {
		visited++
		if (deps.signal?.aborted) throw new Error("shutting down")
		const record = deps.store.spaces[space]
		if (!record?.geo) {
			stats.spacesUnmapped++
			continue
		}
		const geo = record.geo
		const ids = await state.entities(space, () => deps.source.listRelationEntities(space))
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
			state.cursor.inflight = {space, from, to}
			state.save()

			const batchIds = ids.slice(from, to)
			const relations = await deps.source.readRelations(space, batchIds)
			const relsBy = new Map<string, PgRelationRow[]>()
			for (const r of relations) {
				const k = compactId(r.from_entity_id)
				relsBy.set(k, [...(relsBy.get(k) ?? []), r])
			}
			const records = new Map<string, RelationRecord>()
			const statements: string[] = []
			for (const entity of batchIds) {
				const k = compactId(entity)
				const rels = relsBy.get(k) ?? []
				stats.entities++
				if (rels.length === 0) continue
				const e = cid("entity", k)
				const rows = await readClaims(deps.reader, geo, e, sleep)
				const open = new Set(rows.map((r) => r.statement))
				for (const r of rels) {
					stats.relations++
					const sid = cid("statement", compactId(r.id))
					if (deps.store.relationOf(sid)) {
						stats.alreadyRecorded++
						continue
					}
					if (!open.has(sid)) {
						stats.notOpen++
						continue
					}
					const held = facetsOf(rows, sid, () => void stats.facetsUnreadable++)
					records.set(sid, {from: e, prop: cid("entity", compactId(r.type_id)), current: sid, ...(Object.keys(held).length > 0 ? {facets: held} : {})})
					if (Object.keys(held).length > 0) stats.withFacets++
					if (!deps.store.hasStatement(sid)) {
						statements.push(sid)
						stats.statementsMarked++
					}
				}
			}
			stats.toSeed += records.size
			if (!opts.dryRun) {
				// The journals are the feeder's own files; the feeder is stopped (see the Job), so nothing else appends.
				deps.store.commitMarks([], statements, records)
				stats.seeded += records.size
			}
			state.cursor.done[space] = to
			state.cursor.inflight = null
			state.save()
			if (now() - lastProgress >= PROGRESS_EVERY_MS) progress(space, false)
			if (opts.pauseMs > 0) await sleep(opts.pauseMs)
		}
		progress(space, true)
	}
	return stats
}
