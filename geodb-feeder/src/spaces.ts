import {log} from "./log.js"
import {nonceOf} from "./ids.js"
import type {GeoDb} from "./geodb.js"
import type {Metrics} from "./metrics.js"
import {type Store, type SpaceRecord, renderSpaceMap} from "./state.js"

export type SpaceDeps = {
	db: Pick<GeoDb, "waitReady" | "restart">
	store: Store
	metrics: Metrics
	/** Called with the new map text after every change; failures are the publisher's to retry. */
	publish: (text: string) => void
}

/**
 * The geo-db Space for a gaia Space, founding it (a geodb-serve restart) when it is new.
 *
 * Crash-safe: the nonce is deterministic, and the record is saved with `geo: null` before the restart.
 * If the feeder dies mid-founding, the retry restarts with the same nonce, which founds the same Space id,
 * and the Space it did not know before is the one that was founded.
 */
export async function ensureSpace(deps: SpaceDeps, gaiaSpace: string): Promise<SpaceRecord & {geo: string}> {
	const {store} = deps
	const existing = store.spaces[gaiaSpace]
	if (existing?.geo) return existing as SpaceRecord & {geo: string}

	const record: SpaceRecord = existing ?? {geo: null, nonce: nonceOf(gaiaSpace), keyed: false}
	store.spaces[gaiaSpace] = record
	store.saveSpaces()

	await deps.db.waitReady()
	const known = new Set(Object.values(store.spaces).flatMap((s) => (s.geo ? [s.geo] : [])))
	const after = await deps.db.restart(record.nonce)
	deps.metrics.serveRestarts.inc({reason: "found"})
	const fresh = after.filter((id) => !known.has(id))
	if (fresh.length !== 1 || !fresh[0]) {
		throw new Error(`founding ${gaiaSpace}: expected 1 Space geo-db did not already hold, saw ${fresh.length}`)
	}
	record.geo = fresh[0]
	store.saveSpaces()
	deps.metrics.spaces.set({}, Object.values(store.spaces).filter((s) => s.geo).length)
	deps.publish(renderSpaceMap(store.spaces))
	log("info", "founded space", {gaiaSpace, geoSpace: record.geo})
	return record as SpaceRecord & {geo: string}
}
