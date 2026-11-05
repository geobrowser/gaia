/**
 * The feeder's system Spaces (geo-db plan 10.12 K4): one geo-db Space per record family, founded by serve's sealer
 * through the feeder, holding natural-key rows gaia keeps outside any one Space's edits (membership today; votes and
 * config later). A gaia Space's own geo-db Space is not used for them: a Public Space's versions are its chain's
 * (DR-002), and feeder-sequenced versions interleaved with them would break that order.
 *
 * Identity, end to end:
 * - nonce: `recordEntityId(SYSTEM_SPACE_NONCE_KIND, [text("gaia.system-space.<family>")])`, a UUID v4 derived with
 *   the engine's own `record.key` derivation, so it is fixed by the family's name and the same in every
 *   environment and after any loss of feeder state;
 * - geo-db Space id: serve's `foundingSpaceId(sealer, nonce)`, answered by `geodb-client found`. It depends on the
 *   sealer key, so it is per store, exactly like a gaia Space's geo-db id;
 * - recorded in `<DATA_DIR>/feeder/system-spaces.json` (the durable copy, with the `keyed` bit the first apply sets)
 *   and published to the `geodb-space-map` ConfigMap under the key `system-spaces.json`, beside `space-map.json`, as
 *   `{"<family>": {"geo": "<32 hex>", "nonce": "<32 hex>"}}` (the publisher keeps its copy of that value in
 *   `system-spaces.published.json`; the record is this file). It is a separate key because the api's space-map
 *   parser accepts only gaia-to-geo-db id pairs, and these Spaces map no gaia Space.
 *
 * Founded lazily, on the first write of the family, so a family with no consumer turned on costs no Space.
 */
import {existsSync, mkdirSync, readFileSync} from "node:fs"
import {classify, type GeoDb} from "./geodb.js"
import {log} from "./log.js"
import type {Metrics} from "./metrics.js"
import {recordEntityId, textPart} from "./recordKey.js"
import {SYSTEM_SPACE_NONCE_KIND, type SystemFamily} from "./recordKinds.js"
import {type SpaceRecord, writeFileDurable} from "./state.js"

export const SYSTEM_SPACE_MAP_KEY = "system-spaces.json"

export const systemSpaceNonce = (family: SystemFamily): string =>
	recordEntityId(SYSTEM_SPACE_NONCE_KIND, [textPart(`gaia.system-space.${family}`)])

/**
 * geo-db answered a system Space's founding with ResourceExhausted: the Space cap, or one of serve's other
 * per-process limits (the two are not told apart on the wire). Never a reason to skip a row: every row of a
 * family lives in its one Space, so callers let this propagate and the message is retried (the consumer holds
 * its partition, the GeoDbFeederStalled alert fires) or the backfill Job fails.
 */
export class SystemSpaceCapError extends Error {
	constructor(readonly family: SystemFamily) {
		super(`system space ${family} not founded: geo-db answered ResourceExhausted (space cap or a serve limit)`)
	}
}

/** Durable family -> geo-db Space records, apart from `spaces.json` (which maps gaia Spaces only). */
export class SystemSpaces {
	readonly path: string
	readonly records: Partial<Record<SystemFamily, SpaceRecord>>

	constructor(dir: string) {
		mkdirSync(dir, {recursive: true})
		this.path = `${dir}/system-spaces.json`
		this.records = existsSync(this.path) ? JSON.parse(readFileSync(this.path, "utf8")) : {}
	}

	save(): void {
		writeFileDurable(this.path, JSON.stringify(this.records))
	}

	/** The ConfigMap value: founded families only, sorted. */
	render(): string {
		const out: Record<string, {geo: string; nonce: string}> = {}
		for (const [family, rec] of Object.entries(this.records).sort(([a], [b]) => a.localeCompare(b))) {
			if (rec?.geo) out[family] = {geo: rec.geo, nonce: rec.nonce}
		}
		return `${JSON.stringify(out, null, 1)}\n`
	}
}

export type SystemSpaceDeps = {
	db: Pick<GeoDb, "waitReady" | "found">
	sealer: () => Promise<string>
	metrics: Metrics
	systemSpaces: SystemSpaces
	/** Called with the new `system-spaces.json` text after a founding. */
	publishSystem: (text: string) => void
}

/**
 * The geo-db Space for a record family, founding it online the first time. Crash-safe for the same reason
 * `ensureSpace` is: the nonce is fixed and FoundSpace of a live root answers the same id.
 */
export async function ensureSystemSpace(deps: SystemSpaceDeps, family: SystemFamily): Promise<SpaceRecord & {geo: string}> {
	const {systemSpaces} = deps
	const existing = systemSpaces.records[family]
	if (existing?.geo) return existing as SpaceRecord & {geo: string}

	const record: SpaceRecord = existing ?? {geo: null, nonce: systemSpaceNonce(family), keyed: false}
	systemSpaces.records[family] = record
	systemSpaces.save()

	await deps.db.waitReady()
	try {
		record.geo = await deps.db.found(await deps.sealer(), record.nonce)
	} catch (e) {
		if (classify(e instanceof Error ? e.message : String(e)) !== "exhausted") throw e
		log("warn", "system space not founded: geo-db answered ResourceExhausted (space cap or a serve limit); retrying", {family})
		throw new SystemSpaceCapError(family)
	}
	systemSpaces.save()
	deps.publishSystem(systemSpaces.render())
	log("info", "founded system space", {family, geoSpace: record.geo})
	return record as SpaceRecord & {geo: string}
}
