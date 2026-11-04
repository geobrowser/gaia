import {readFileSync} from "node:fs"
import {log} from "./log.js"
import {nonceOf} from "./ids.js"
import {classify, type GeoDb} from "./geodb.js"
import type {Metrics} from "./metrics.js"
import {type Store, type SpaceRecord, backfilledSpaces, renderSpaceMap} from "./state.js"

export type SpaceDeps = {
	db: Pick<GeoDb, "waitReady" | "found">
	/** serve's sealer key id (public), published by the supervisor; resolves once it is known. */
	sealer: () => Promise<string>
	store: Store
	metrics: Metrics
	/** Called with the new map text after every change; failures are the publisher's to retry. */
	publish: (text: string) => void
}

/** geo-db refused to found this gaia Space: it holds as many Spaces as `spaces` in its serve config allows. */
export class SpaceCapError extends Error {
	constructor(readonly gaiaSpace: string) {
		super(`space ${gaiaSpace} not founded: geo-db is at its space cap`)
	}
}

/** Gaia Spaces geo-db refused to found, per store, for the life of the process (a restart retries them). */
const capRefused = new WeakMap<Store, Set<string>>()
const refusedFor = (store: Store): Set<string> => {
	let set = capRefused.get(store)
	if (!set) capRefused.set(store, (set = new Set()))
	return set
}

/** Dead-letter the message that needed a Space geo-db cannot found, and move on. */
export function deadLetterSpaceCap(
	deps: Pick<SpaceDeps, "store" | "metrics">,
	meta: Record<string, unknown>,
	err: SpaceCapError,
): void {
	deps.store.deadLetter({...meta, space: err.gaiaSpace, kind: "space_cap", err: err.message})
	deps.metrics.deadLetters.inc({kind: "space_cap"})
}

/**
 * The geo-db Space for a gaia Space, founding it online when it is new: wire FoundSpace under a deterministic
 * nonce, with serve still running. Calls after it are new connections, so they hold grants over the Space.
 *
 * Crash-safe: the nonce is deterministic and founding is idempotent, so a retry after a crash founds nothing
 * new and answers the same id.
 *
 * Throws `SpaceCapError` when geo-db refuses the founding as exhausted (its Space cap); the first refusal per
 * gaia Space is logged, and later calls throw without asking geo-db. Any other failure propagates for a retry.
 */
export async function ensureSpace(deps: SpaceDeps, gaiaSpace: string): Promise<SpaceRecord & {geo: string}> {
	const {store} = deps
	const existing = store.spaces[gaiaSpace]
	if (existing?.geo) return existing as SpaceRecord & {geo: string}

	const refused = refusedFor(store)
	if (refused.has(gaiaSpace)) throw new SpaceCapError(gaiaSpace)

	const record: SpaceRecord = existing ?? {geo: null, nonce: nonceOf(gaiaSpace), keyed: false}
	store.spaces[gaiaSpace] = record
	store.saveSpaces()

	await deps.db.waitReady()
	try {
		record.geo = await deps.db.found(await deps.sealer(), record.nonce)
	} catch (e) {
		if (classify(e instanceof Error ? e.message : String(e)) !== "exhausted") throw e
		refused.add(gaiaSpace)
		log("warn", "space not founded: geo-db is at its space cap", {gaiaSpace})
		throw new SpaceCapError(gaiaSpace)
	}
	store.saveSpaces()
	deps.metrics.spaces.set({}, Object.values(store.spaces).filter((s) => s.geo).length)
	deps.publish(renderSpaceMap(store.spaces, backfilledSpaces(store.dir)))
	log("info", "founded space", {gaiaSpace, geoSpace: record.geo})
	return record as SpaceRecord & {geo: string}
}

/** serve's sealer key id (public). The supervisor publishes it to the geodb-space-map ConfigMap, mounted at `path`. */
export async function readSealer(path: string, sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))): Promise<string> {
	for (;;) {
		try {
			const id = readFileSync(path, "utf8").trim()
			if (/^[0-9a-f]{64}$/.test(id)) return id
		} catch {
			// not published yet
		}
		log("warn", "waiting for serve's sealer id", {path})
		await sleep(5000)
	}
}
