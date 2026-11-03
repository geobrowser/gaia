import {log} from "./log.js"
import {nonceOf} from "./ids.js"
import type {GeoDb} from "./geodb.js"
import type {Metrics} from "./metrics.js"
import {type Store, type SpaceRecord, renderSpaceMap} from "./state.js"

export type SpaceDeps = {
	db: Pick<GeoDb, "waitReady" | "found">
	/** serve's sealer key id (public), published by the supervisor; resolves once it is known. */
	sealer: () => Promise<string>
	store: Store
	metrics: Metrics
	/** Called with the new map text after every change; failures are the publisher's to retry. */
	publish: (text: string) => void
}

/**
 * The geo-db Space for a gaia Space, founding it online when it is new: wire FoundSpace under a deterministic
 * nonce, with serve still running. Calls after it are new connections, so they hold grants over the Space.
 *
 * Crash-safe: the nonce is deterministic and founding is idempotent, so a retry after a crash founds nothing
 * new and answers the same id.
 */
export async function ensureSpace(deps: SpaceDeps, gaiaSpace: string): Promise<SpaceRecord & {geo: string}> {
	const {store} = deps
	const existing = store.spaces[gaiaSpace]
	if (existing?.geo) return existing as SpaceRecord & {geo: string}

	const record: SpaceRecord = existing ?? {geo: null, nonce: nonceOf(gaiaSpace), keyed: false}
	store.spaces[gaiaSpace] = record
	store.saveSpaces()

	await deps.db.waitReady()
	record.geo = await deps.db.found(await deps.sealer(), record.nonce)
	store.saveSpaces()
	deps.metrics.spaces.set({}, Object.values(store.spaces).filter((s) => s.geo).length)
	deps.publish(renderSpaceMap(store.spaces))
	log("info", "founded space", {gaiaSpace, geoSpace: record.geo})
	return record as SpaceRecord & {geo: string}
}
