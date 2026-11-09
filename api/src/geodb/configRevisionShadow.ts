/**
 * The For you config revision twin (zero-Postgres critical path). `personalization.feed_config_revision` is a one-row counter
 * the plpgsql trigger `bump_feed_config_revision` increments, once per statement, whenever `for_you_config`,
 * `interest_signal_weights`, `interest_config` or `entity_ranking_config` change. The geodb-feeder deliberately does not write
 * it (geodb-feeder/src/forYouConfigKeyed.ts: "derived"); this file is the derivation.
 *
 * What the revision is for. Its only consumer in this repo is `readSettings` (forYou/queries.ts), which hands it to
 * `forYouVersion` (forYou/router.ts): the served ranking `version` is `for-you-<code>.<revision>[+seen.<n>]`, and the same number
 * is echoed as `ranking.configRevision`. Nothing here orders, subtracts or compares two revisions: it is an opaque
 * "which config produced this ranking" tag, so a caller (the web app's server, which sends one scroll's pages with a pinned
 * `asOf`) can tell that two pages, or a page and a later page, were ranked under the same config. The contract is therefore
 * "changed since X" (equality), not "monotonic". It need not be dense or ordered; it must differ whenever any value the
 * ranking reads differs, and must not differ when nothing did.
 *
 * Derivation. A SHA-256 over every statement (property, value) on the records the config feeder mirrors into the `config`
 * system Space: the `for_you_config` record, the `entity_ranking_config` record, the `interest_config` record and one
 * `interest_signal_weights` record per kind. Sorted, so it does not depend on read order. It reads whole cards, not named
 * columns, so a column added to a table later is covered with no change here. `revision` is its first 48 bits as an integer
 * (below 2^53), so the response field and the version string keep their type.
 *
 * Differences from the counter, on purpose: a statement that rewrites the same values (an UPDATE that changes nothing, a
 * TRUNCATE-and-reinsert of the same rows) bumps the counter and not the digest, which is the better answer to "changed since
 * X"; and the digest can return to an earlier value when an edit is reverted, where a counter never would (a ranking served
 * under A, then B, then A again is the same config, and equal versions say so). A caller that needs "newer than" cannot use
 * it; none in this repo does.
 *
 * Blind spots: (1) the `interest_signal_weights` kinds are enumerated from the six the schema seeds (0100) and the readers
 * name, since a record cannot be listed by Space from here; a kind added by operator SQL is not seen until it is added to
 * `INTEREST_SIGNAL_KINDS` (the shadow then reports `revision_only` for that edit). (2) The records are only as fresh as the
 * config feeder's backfill Jobs (there is no event stream for these tables), so the digest lags a Postgres edit until the
 * Job is rerun.
 *
 * The twin serves nothing: GEODB_CONFIG_REVISION_SHADOW is off by default and `readSettings` stays on Postgres. When on, a
 * background timer reads Postgres' revision and the digest every `intervalMs` and compares whether each CHANGED since the
 * previous pass (their values are not comparable):
 *
 * `gaia_api_config_revision_shadow_total{result}`:
 *  - baseline: the first pass, or the first after a failed or unsupported one; nothing to compare;
 *  - match_unchanged: neither moved;
 *  - match_changed: both moved;
 *  - revision_only: the counter moved and the digest did not (a no-op statement, a kind outside the list, or geo-db lagging:
 *    the next pass then shows digest_only);
 *  - digest_only: the digest moved and the counter did not (geo-db caught up after revision_only, or a backfill wrote values
 *    Postgres does not hold);
 *  - unsupported: the `config` system Space is not in the map, or one of the three singleton records is absent;
 *  - failed: a geo-db read failed;
 *  - reference_timeout / reference_failed: the Postgres counter read hit its statement timeout, or failed otherwise
 *    (referenceSide.ts); like any failed or unsupported pass, the next one is a baseline.
 * The timer is `createIntervalSampler` (geodb/intervalSampler.ts), shared with the other config shadows.
 * Logs counts only, never a key or a value.
 */

import {createHash} from "node:crypto"
import {sql} from "drizzle-orm"
import {forYouConfigRecordId} from "../forYou/configShadow"
import {entityRankingConfigRecordId} from "../forYou/entityRankingConfigShadow"
import type {Database} from "../forYou/queries"
import {intPart, recordEntityId, textPart} from "../proposals/governance/recordKey"
import type {AnySystemSpaceLookup} from "../proposals/governance/systemSpaces"
import type {GeoCardRow} from "./card"
import {type GeoDbCardClient, GeoDbError} from "./client"
import {ConfigSpaceMissing} from "./entityFeedConfigShadow"
import {createIntervalSampler, type IntervalSampler, readIntervalSamplerConfig} from "./intervalSampler"
import {failureResult, viaReference} from "./referenceSide"

/** The feeder's pins (geodb-feeder/src/recordKinds.ts); `__tests__/configRevisionShadow.test.ts` holds them to that file. */
export const INTEREST_CONFIG_KIND = "e7ca641275a840bebb7ac20424a5deae"
export const INTEREST_SIGNAL_WEIGHT_KIND = "1c35f5a4877f474a8d13835f8e05205d"

/** The kinds `interest_signal_weights` is seeded with (api/drizzle/0100) and its readers name. */
export const INTEREST_SIGNAL_KINDS = ["vote", "comment", "debate", "follow", "interested", "not_interested"] as const

export const interestConfigRecordId = (): string => recordEntityId(INTEREST_CONFIG_KIND, [intPart(1)])
export const interestSignalWeightRecordId = (kind: string): string =>
	recordEntityId(INTEREST_SIGNAL_WEIGHT_KIND, [textPart(kind)])

/** Thrown when one of the three single-row config records has no statements at all. */
export class ConfigRecordMissing extends Error {}

export type ConfigRevision = {
	/** SHA-256 hex over every statement of the config records. */
	digest: string
	/** The first 48 bits of the digest as an integer: what takes the counter's place in `forYouVersion`. */
	revision: number
}

/** The digest of the records' statements. Pure: depends on the set of (record, property, value) alone, not on order. */
export function digestConfigRecords(records: {id: string; rows: GeoCardRow[]}[]): ConfigRevision {
	const lines = records
		.flatMap((r) => r.rows.map((row) => `${r.id}\t${row.prop}\t${row.raw}`))
		.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))
	const digest = createHash("sha256").update(lines.join("\n")).digest("hex")
	return {digest, revision: Number.parseInt(digest.slice(0, 12), 16)}
}

export type ConfigRevisionReader = {
	/** The derived revision, or rejects: ConfigSpaceMissing, ConfigRecordMissing, GeoDbError. */
	read(): Promise<ConfigRevision>
}

export function createConfigRevisionReader(deps: {
	geoDb: Pick<GeoDbCardClient, "readCard">
	systemSpaceOf: AnySystemSpaceLookup
}): ConfigRevisionReader {
	return {
		async read() {
			const geoSpaceId = await deps.systemSpaceOf("config")
			if (!geoSpaceId) throw new ConfigSpaceMissing()
			const singletons = [forYouConfigRecordId(), entityRankingConfigRecordId(), interestConfigRecordId()]
			const ids = [...singletons, ...INTEREST_SIGNAL_KINDS.map(interestSignalWeightRecordId)]
			const records = await Promise.all(
				ids.map(async (id) => ({id, rows: await deps.geoDb.readCard({geoSpaceId, entityId: id})})),
			)
			if (records.slice(0, singletons.length).some((r) => r.rows.length === 0)) throw new ConfigRecordMissing()
			return digestConfigRecords(records)
		},
	}
}

export type ConfigRevisionShadowConfig = {enabled: boolean; intervalMs: number}

export function readConfigRevisionShadowConfig(env: Record<string, string | undefined>): ConfigRevisionShadowConfig {
	return readIntervalSamplerConfig(
		env,
		"GEODB_CONFIG_REVISION_SHADOW",
		"GEODB_CONFIG_REVISION_SHADOW_INTERVAL_MS",
		30_000,
	)
}

export const RESULTS = [
	"baseline",
	"match_unchanged",
	"match_changed",
	"revision_only",
	"digest_only",
	"unsupported",
	"failed",
	"reference_timeout",
	"reference_failed",
] as const
export type Result = (typeof RESULTS)[number]

/** Whether each side moved since the previous pass, as a result. Pure. */
export function compareMovement(counterMoved: boolean, digestMoved: boolean): Result {
	if (counterMoved && digestMoved) return "match_changed"
	if (counterMoved) return "revision_only"
	if (digestMoved) return "digest_only"
	return "match_unchanged"
}

let active = false
const results = new Map<Result, number>()
const bump = (result: Result): void => {
	results.set(result, (results.get(result) ?? 0) + 1)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once on, every series exists at zero. */
export function renderConfigRevisionShadowMetrics(): string {
	if (!active) return ""
	return [
		"# HELP gaia_api_config_revision_shadow_total Whether the derived For you config digest from geo-db's config Space moved between passes exactly when Postgres' feed_config_revision did, by result: baseline, match_unchanged, match_changed, revision_only (counter moved, digest did not), digest_only (digest moved, counter did not), unsupported, failed (geo-db) or reference_timeout / reference_failed (the Postgres counter read).",
		"# TYPE gaia_api_config_revision_shadow_total counter",
		...RESULTS.map((r) => `gaia_api_config_revision_shadow_total{result="${r}"} ${results.get(r) ?? 0}`),
	].join("\n")
}

export function __resetConfigRevisionShadowMetricsForTests(): void {
	active = false
	results.clear()
}

/** Postgres' counter. */
export async function readPostgresRevision(db: Database): Promise<number> {
	const result = await db.execute(sql`SELECT revision FROM personalization.feed_config_revision`)
	const row = ((result as {rows?: Record<string, unknown>[]}).rows ?? [])[0]
	const n = Number(row?.revision)
	if (!Number.isInteger(n)) throw new Error("feed_config_revision has no row")
	return n
}

export type ConfigRevisionShadow = IntervalSampler

export function createConfigRevisionShadow(deps: {
	config: ConfigRevisionShadowConfig
	db: Database
	reader: ConfigRevisionReader
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	readRevision?: typeof readPostgresRevision
}): ConfigRevisionShadow {
	const {config, db, reader, log} = deps
	const readRevision = deps.readRevision ?? readPostgresRevision
	active = true
	let last: {counter: number; digest: string} | null = null
	let logged = 0

	async function pass(): Promise<void> {
		try {
			const counter = await viaReference(readRevision(db))
			const {digest} = await reader.read()
			if (last === null) bump("baseline")
			else {
				const outcome = compareMovement(counter !== last.counter, digest !== last.digest)
				bump(outcome)
				if ((outcome === "revision_only" || outcome === "digest_only") && logged < 20) {
					logged++
					log.warn("config revision shadow: counter and digest moved differently", {outcome})
				}
			}
			last = {counter, digest}
		} catch (error) {
			last = null
			if (error instanceof ConfigSpaceMissing || error instanceof ConfigRecordMissing) bump("unsupported")
			else {
				const result = failureResult(error)
				bump(result)
				if (result !== "failed" || !(error instanceof GeoDbError))
					log.warn("config revision shadow failed", {result, error: String(error)})
			}
		}
	}

	return createIntervalSampler({
		enabled: config.enabled,
		intervalMs: config.intervalMs,
		pass,
		log,
		label: "config revision shadow",
	})
}
