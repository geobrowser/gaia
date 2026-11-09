/**
 * The shadow of the per-user readers' twins (privateReaders.ts): GEODB_PRIVATE_READERS_SHADOW, off by default. It serves
 * nothing; `readWeights`, `readSeen` (forYou/queries.ts) and `readOwnWeights` (pairFit/queries.ts) stay on Postgres, and the
 * For you ranked list itself is not touched.
 *
 * Each pass, on a timer (`createIntervalSampler`, default every five minutes, never on a request), picks ONE user at random
 * from `user_topic_signals` and one from `feed_user_seen_daily`, then for each:
 *  - weights: Postgres `user_topic_weights(user, for_you_config.max_topics, as_of)` (what `readWeights` calls, without the
 *    public name lookups) against the twin: one scan of the user's group in restricted collection 28, one scan of collection
 *    23 per own topic of positive weight, and the `interest_config`, six `interest_signal_weights` and `for_you_config`
 *    records from the `config` Space. The decay is applied at the same `as_of` on both sides.
 *  - own_weights: the same two lists filtered to `own_weight > 0` (`readOwnWeights`), counted on its own.
 *  - seen: Postgres `feed_user_seen(user, now − 7 days)` against one scan of the user's group in restricted collection 29;
 *    the per-item counts must be equal.
 *
 * The restricted collections are read with a key of their own, GEODB_PRIVATE_KEY_PATH, whose principal carries
 * `derived_read=28,29` (geodb/supervisor.sh, GEODB_PRIVATE_READER_KEYID; needs serve and client at geo-db 9b871ed4e4 or
 * later). The shared reader key is never given that grant. Without the key path the shadow is not mounted.
 *
 * `gaia_api_private_readers_shadow_total{reader, result}`, reader weights | own_weights | seen:
 *  - match: the same rows (weights within 1e-9 relative; a top-N boundary tie may swap which topic is last);
 *  - mismatch: anything else, including a lag between the two-minute sweep and its mirror;
 *  - absent: geo-db holds no row in the user's group (the mirror is off or has not reached the user), not a disagreement;
 *  - restricted: geo-db refused the scan as `NotFoundOrRestricted`: the key's `derived_read=` grant is not applied;
 *  - unsupported: the twin cannot answer this user exactly (`..._unsupported_total{reason}`: truncated, the group has
 *    more rows than one page of 512; too_many_topics, more than 128 collection 23 scans; config_missing; ambiguous_config;
 *    busy, the previous pass still running);
 *  - malformed: a row is not the mirror's shape;
 *  - failed: a geo-db read failed otherwise;
 *  - reference_timeout / reference_failed: the Postgres side (referenceSide.ts).
 *
 * Privacy: these are per-user rows. Nothing here logs or exports a user id, topic, item or value: logs and metrics carry
 * counts and labels only, and a pass reads one sampled user's group and nothing else of collection 28 or 29.
 */

import {sql} from "drizzle-orm"
import {decodeForYouConfig, forYouConfigRecordId} from "../forYou/configShadow"
import type {Database} from "../forYou/queries"
import type {AnySystemSpaceLookup} from "../proposals/governance/systemSpaces"
import type {GeoDbCardClient} from "./client"
import {INTEREST_SIGNAL_KINDS, interestConfigRecordId, interestSignalWeightRecordId} from "./configRevisionShadow"
import {type DerivedScan, type DerivedScanReader, MAX_SCAN_ROWS, refusalToken} from "./derived"
import {createIntervalSampler, type IntervalSampler, readIntervalSamplerConfig} from "./intervalSampler"
import {
	AmbiguousConfig,
	close,
	decodeInterestConfig,
	decodeNeighbours,
	decodeSeen,
	decodeSignals,
	decodeSignalWeight,
	FEED_USER_SEEN_COLLECTION,
	feedUserSeen,
	groupHex,
	type InterestConfig,
	MalformedRow,
	type Neighbour,
	ownContributions,
	ownTopicWeights,
	type SignalWeights,
	seenCounts,
	seenPageComplete,
	spreadingTopics,
	TOPIC_COOCCURRENCE_COLLECTION,
	type TopicWeightRow,
	USER_TOPIC_SIGNALS_COLLECTION,
	userTopicContributions,
	userTopicWeights,
} from "./privateReaders"
import {failureResult, viaReference} from "./referenceSide"

/** The seen window the shadow asks for; the route's comes from the request (`seen.days`), a week is its usual. */
export const SEEN_SAMPLE_DAYS = 7
/** Collection 23 scans one weights sample may make: one per own topic of positive weight. */
export const MAX_SPREAD_SCANS = 128

export type PrivateReadersShadowConfig = {enabled: boolean; intervalMs: number; keyPath: string | null}

export function readPrivateReadersShadowConfig(env: Record<string, string | undefined>): PrivateReadersShadowConfig {
	const base = readIntervalSamplerConfig(
		env,
		"GEODB_PRIVATE_READERS_SHADOW",
		"GEODB_PRIVATE_READERS_SHADOW_INTERVAL_MS",
	)
	const keyPath = env.GEODB_PRIVATE_KEY_PATH?.trim() || null
	return {...base, keyPath}
}

export const READERS = ["weights", "own_weights", "seen"] as const
export type Reader = (typeof READERS)[number]
export const RESULTS = [
	"match",
	"mismatch",
	"absent",
	"restricted",
	"unsupported",
	"malformed",
	"failed",
	"reference_timeout",
	"reference_failed",
] as const
export type Result = (typeof RESULTS)[number]
export const UNSUPPORTED_REASONS = [
	"truncated",
	"too_many_topics",
	"config_missing",
	"ambiguous_config",
	"busy",
] as const
export type UnsupportedReason = (typeof UNSUPPORTED_REASONS)[number]

class Unsupported extends Error {
	constructor(readonly reason: UnsupportedReason) {
		super(reason)
	}
}
class Absent extends Error {}
class Restricted extends Error {}

let active = false
const results = new Map<string, number>()
const unsupported = new Map<string, number>()
const bump = (map: Map<string, number>, key: string): void => {
	map.set(key, (map.get(key) ?? 0) + 1)
}

/** Empty until the shadow is mounted, so a flag-off deploy exports nothing; once on, every series exists at zero. */
export function renderPrivateReadersShadowMetrics(): string {
	if (!active) return ""
	return [
		"# HELP gaia_api_private_readers_shadow_total Whether the twin of a per-user reader (weights: user_topic_weights; own_weights: its own_weight > 0 rows; seen: feed_user_seen), computed from geo-db's restricted collections 28 / 29 for one sampled user, equals Postgres, by result: match, mismatch, absent (geo-db has no rows for the user), restricted (the scan was refused: the private reader grant is not applied), unsupported, malformed, failed (geo-db) or reference_timeout / reference_failed (Postgres). Counts only.",
		"# TYPE gaia_api_private_readers_shadow_total counter",
		...READERS.flatMap((reader) =>
			RESULTS.map(
				(r) =>
					`gaia_api_private_readers_shadow_total{reader="${reader}",result="${r}"} ${results.get(`${reader}|${r}`) ?? 0}`,
			),
		),
		"# HELP gaia_api_private_readers_shadow_unsupported_total Why a per-user reader sample was unsupported: truncated (more rows than one scan page), too_many_topics, config_missing, ambiguous_config, busy.",
		"# TYPE gaia_api_private_readers_shadow_unsupported_total counter",
		...UNSUPPORTED_REASONS.map(
			(r) => `gaia_api_private_readers_shadow_unsupported_total{reason="${r}"} ${unsupported.get(r) ?? 0}`,
		),
	].join("\n")
}

export function __resetPrivateReadersShadowMetricsForTests(): void {
	active = false
	results.clear()
	unsupported.clear()
}

// ---- comparisons (pure) ---------------------------------------------------------------------------------------------------

/** Same rows in the same order, values within `close`; a tie at the top-N boundary may change which topic is last. */
export function sameWeights(postgres: TopicWeightRow[], twin: TopicWeightRow[]): boolean {
	if (postgres.length !== twin.length) return false
	for (let i = 0; i < postgres.length; i++)
		if (!close((postgres[i] as TopicWeightRow).weight, (twin[i] as TopicWeightRow).weight)) return false
	const byTopic = new Map(twin.map((r) => [r.topicId, r]))
	const boundary = postgres[postgres.length - 1]?.weight
	for (const p of postgres) {
		const t = byTopic.get(p.topicId)
		if (!t) {
			if (boundary !== undefined && close(p.weight, boundary)) continue
			return false
		}
		if (
			!close(p.ownWeight, t.ownWeight) ||
			!close(p.relatedWeight, t.relatedWeight) ||
			p.topKind !== t.topKind ||
			p.topKindCount !== t.topKindCount ||
			p.topObjectId !== t.topObjectId ||
			p.relatedViaTopicId !== t.relatedViaTopicId
		)
			return false
	}
	return true
}

export function sameOwnWeights(
	postgres: {topicId: string; weight: number}[],
	twin: {topicId: string; weight: number}[],
): boolean {
	if (postgres.length !== twin.length) return false
	const byTopic = new Map(twin.map((r) => [r.topicId, r.weight]))
	const boundary = postgres[postgres.length - 1]?.weight
	return postgres.every((p) => {
		const w = byTopic.get(p.topicId)
		return w === undefined ? boundary !== undefined && close(p.weight, boundary) : close(p.weight, w)
	})
}

export function sameCounts(postgres: Map<string, number>, twin: Map<string, number>): boolean {
	if (postgres.size !== twin.size) return false
	for (const [item, n] of postgres) if (twin.get(item) !== n) return false
	return true
}

// ---- Postgres reference ----------------------------------------------------------------------------------------------------

const rowsOf = (result: unknown): Record<string, unknown>[] => (result as {rows?: Record<string, unknown>[]}).rows ?? []
const numOrNull = (v: unknown): number | null => (v === null || v === undefined ? null : Number(v))
const strOrNull = (v: unknown): string | null => (v === null || v === undefined ? null : String(v))

export type ReferenceSide = {
	sampleSignalsUser(db: Database): Promise<string | null>
	sampleSeenUser(db: Database): Promise<string | null>
	weights(db: Database, userId: string, asOf: Date): Promise<TopicWeightRow[]>
	seen(db: Database, userId: string, since: Date): Promise<Map<string, number>>
}

/** The Postgres side: one random user per table, and the two functions the readers call, for that user only. */
export const postgresReference: ReferenceSide = {
	async sampleSignalsUser(db) {
		const r = await db.execute(
			sql`SELECT user_id FROM personalization.user_topic_signals ORDER BY random() LIMIT 1`,
		)
		return strOrNull(rowsOf(r)[0]?.user_id)
	},
	async sampleSeenUser(db) {
		const r = await db.execute(
			sql`SELECT user_id FROM personalization.feed_user_seen_daily ORDER BY random() LIMIT 1`,
		)
		return strOrNull(rowsOf(r)[0]?.user_id)
	},
	async weights(db, userId, asOf) {
		const r = await db.execute(sql`
			SELECT w.topic_id, w.weight, w.own_weight, w.related_weight, w.top_kind, w.top_kind_count,
			       w.top_object_id, w.related_via_topic_id
			FROM personalization.user_topic_weights(
			       ${userId}::uuid,
			       (SELECT max_topics FROM personalization.for_you_config),
			       ${asOf.toISOString()}::timestamptz) w
		`)
		return rowsOf(r).map((row) => ({
			topicId: String(row.topic_id),
			weight: Number(row.weight),
			ownWeight: Number(row.own_weight),
			relatedWeight: Number(row.related_weight),
			topKind: String(row.top_kind),
			topKindCount: numOrNull(row.top_kind_count),
			topObjectId: strOrNull(row.top_object_id),
			relatedViaTopicId: strOrNull(row.related_via_topic_id),
		}))
	},
	async seen(db, userId, since) {
		const r = await db.execute(
			sql`SELECT item_id, seen_count FROM personalization.feed_user_seen(${userId}::uuid, ${since.toISOString()}::timestamptz)`,
		)
		const counts = new Map<string, number>()
		for (const row of rowsOf(r)) {
			const views = Number(row.seen_count)
			if (row.item_id && views > 0) counts.set(String(row.item_id), views)
		}
		return counts
	},
}

// ---- geo-db twin -------------------------------------------------------------------------------------------------------------

/** A restricted scan; a refusal as a hidden row is `Restricted`, an empty group `Absent`. */
async function scanRestricted(
	reader: Pick<DerivedScanReader, "scan">,
	collection: number,
	userId: string,
): Promise<DerivedScan> {
	let scan: DerivedScan
	try {
		scan = await reader.scan(collection, groupHex(userId), MAX_SCAN_ROWS)
	} catch (error) {
		if (refusalToken(error) === "NotFoundOrRestricted") throw new Restricted()
		throw error
	}
	if (scan.rows.length === 0) throw new Absent()
	return scan
}

export type TwinConfig = {interest: InterestConfig; weights: SignalWeights; maxTopics: number}

/** The interest config, the six signal weights and max_topics from the `config` Space's records. */
export async function readTwinConfig(
	geoDb: Pick<GeoDbCardClient, "readCard">,
	systemSpaceOf: AnySystemSpaceLookup,
): Promise<TwinConfig> {
	const geoSpaceId = await systemSpaceOf("config")
	if (!geoSpaceId) throw new Unsupported("config_missing")
	const card = (entityId: string) => geoDb.readCard({geoSpaceId, entityId})
	const [interestRows, forYouRows, ...weightRows] = await Promise.all([
		card(interestConfigRecordId()),
		card(forYouConfigRecordId()),
		...INTEREST_SIGNAL_KINDS.map((k) => card(interestSignalWeightRecordId(k))),
	])
	let interest: InterestConfig | null
	let forYou: ReturnType<typeof decodeForYouConfig>
	const weights: SignalWeights = new Map()
	try {
		interest = decodeInterestConfig(interestRows ?? [])
		forYou = decodeForYouConfig(forYouRows ?? [])
		INTEREST_SIGNAL_KINDS.forEach((kind, i) => {
			const w = decodeSignalWeight(weightRows[i] ?? [])
			if (w) weights.set(kind, w)
		})
	} catch {
		throw new Unsupported("ambiguous_config")
	}
	// A kind Postgres seeds but geo-db has no record of would drop silently from the join; call it unsupported instead.
	if (!interest || !forYou || weights.size !== INTEREST_SIGNAL_KINDS.length) throw new Unsupported("config_missing")
	return {interest, weights, maxTopics: forYou.maxTopics}
}

/** The weights twin for one user. Rejects Absent, Restricted, Unsupported, MalformedRow or a geo-db error. */
export async function twinWeights(
	reader: Pick<DerivedScanReader, "scan">,
	config: TwinConfig,
	userId: string,
	asOf: Date,
): Promise<TopicWeightRow[]> {
	const scan = await scanRestricted(reader, USER_TOPIC_SIGNALS_COLLECTION, userId)
	if (scan.more) throw new Unsupported("truncated")
	const signals = decodeSignals(userId, scan.rows)
	const spreading = spreadingTopics(ownContributions(signals, config.interest, config.weights, asOf), config.interest)
	if (spreading.length > MAX_SPREAD_SCANS) throw new Unsupported("too_many_topics")
	const neighbours = new Map<string, Neighbour[]>()
	for (const topicId of spreading) {
		const n = await reader.scan(TOPIC_COOCCURRENCE_COLLECTION, groupHex(topicId), MAX_SCAN_ROWS)
		if (n.more) throw new Unsupported("truncated")
		neighbours.set(topicId, decodeNeighbours(topicId, n.rows))
	}
	return userTopicWeights(
		userTopicContributions(signals, config.interest, config.weights, neighbours, asOf),
		config.maxTopics,
	)
}

/** The seen twin for one user: `readSeen`'s counts. */
export async function twinSeen(
	reader: Pick<DerivedScanReader, "scan">,
	userId: string,
	since: Date,
): Promise<Map<string, number>> {
	const scan = await scanRestricted(reader, FEED_USER_SEEN_COLLECTION, userId)
	if (!seenPageComplete(scan, since)) throw new Unsupported("truncated")
	return seenCounts(feedUserSeen(decodeSeen(userId, scan.rows), since))
}

// ---- the shadow ----------------------------------------------------------------------------------------------------------------

export type PrivateReadersShadow = IntervalSampler

export function createPrivateReadersShadow(deps: {
	config: PrivateReadersShadowConfig
	db: Database
	/** A derived reader opened with GEODB_PRIVATE_KEY_PATH, the key whose principal has `derived_read=28,29`. */
	reader: Pick<DerivedScanReader, "scan">
	geoDb: Pick<GeoDbCardClient, "readCard">
	systemSpaceOf: AnySystemSpaceLookup
	log: {warn: (message: string, data?: Record<string, unknown>) => void}
	reference?: ReferenceSide
	now?: () => Date
}): PrivateReadersShadow {
	const {config, db, reader, geoDb, systemSpaceOf, log} = deps
	const reference = deps.reference ?? postgresReference
	const now = deps.now ?? (() => new Date())
	active = true
	let logged = 0
	const warn = (message: string, data?: Record<string, unknown>) => {
		if (logged >= 20) return
		logged++
		log.warn(message, data)
	}

	/** Count an error as its result; returns the result so the caller can count it for every reader it stood for. */
	function classify(error: unknown): {result: Result; reason?: UnsupportedReason} {
		if (error instanceof Unsupported) return {result: "unsupported", reason: error.reason}
		if (error instanceof Absent) return {result: "absent"}
		if (error instanceof Restricted) return {result: "restricted"}
		if (error instanceof MalformedRow) return {result: "malformed"}
		if (error instanceof AmbiguousConfig) return {result: "unsupported", reason: "ambiguous_config"}
		return {result: failureResult(error)}
	}

	function count(readers: Reader[], outcome: {result: Result; reason?: UnsupportedReason}): void {
		for (const reader of readers) bump(results, `${reader}|${outcome.result}`)
		if (outcome.reason) bump(unsupported, outcome.reason)
		if (outcome.result === "mismatch") warn("private readers shadow: twin differs", {readers})
		else if (outcome.result === "malformed")
			warn("private readers shadow: a row is not the mirror's shape", {readers})
		else if (
			outcome.result === "reference_timeout" ||
			outcome.result === "reference_failed" ||
			outcome.result === "failed"
		)
			warn("private readers shadow failed", {readers, result: outcome.result})
	}

	async function weightsSample(): Promise<void> {
		try {
			const userId = await viaReference(reference.sampleSignalsUser(db))
			if (!userId) return
			const asOf = now()
			const twinConfig = await readTwinConfig(geoDb, systemSpaceOf)
			const [postgres, twin] = await Promise.all([
				viaReference(reference.weights(db, userId, asOf)),
				twinWeights(reader, twinConfig, userId, asOf),
			])
			count(["weights"], {result: sameWeights(postgres, twin) ? "match" : "mismatch"})
			const pgOwn = postgres
				.filter((r) => r.ownWeight > 0)
				.map((r) => ({topicId: r.topicId, weight: r.ownWeight}))
			count(["own_weights"], {result: sameOwnWeights(pgOwn, ownTopicWeights(twin)) ? "match" : "mismatch"})
		} catch (error) {
			count(["weights", "own_weights"], classify(error))
		}
	}

	async function seenSample(): Promise<void> {
		try {
			const userId = await viaReference(reference.sampleSeenUser(db))
			if (!userId) return
			const since = new Date(now().getTime() - SEEN_SAMPLE_DAYS * 86400 * 1000)
			const [postgres, twin] = await Promise.all([
				viaReference(reference.seen(db, userId, since)),
				twinSeen(reader, userId, since),
			])
			count(["seen"], {result: sameCounts(postgres, twin) ? "match" : "mismatch"})
		} catch (error) {
			count(["seen"], classify(error))
		}
	}

	return createIntervalSampler({
		enabled: config.enabled && config.keyPath !== null,
		intervalMs: config.intervalMs,
		pass: async () => {
			await weightsSample()
			await seenSample()
		},
		onBusy: () => {
			for (const reader of READERS) bump(results, `${reader}|unsupported`)
			bump(unsupported, "busy")
		},
		log,
		label: "private readers shadow",
	})
}
