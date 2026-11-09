/**
 * Twins of the four per-user `personalization` readers (api/drizzle/0100 and 0109), computed from geo-db's restricted
 * derived collections instead of Postgres. Pure: the inputs are decoded scan rows and config values, the outputs the rows
 * the SQL functions return. Nothing here serves a request; `privateReadersShadow.ts` compares them with Postgres.
 *
 *  - `userTopicContributions`  = `personalization.user_topic_contributions(user, as_of)`
 *  - `userTopicWeights`        = `personalization.user_topic_weights(user, max_topics, as_of)` (forYou `readWeights`)
 *  - `ownTopicWeights`         = the same, `WHERE own_weight > 0` (pairFit `readOwnWeights`)
 *  - `userTopicInterestReasons` = `personalization.user_topic_interest_reasons(user, topic, as_of)`
 *  - `feedUserSeen`            = `personalization.feed_user_seen(user, since)` (forYou `readSeen`'s counts)
 *
 * Where the rows live (written by ranking-indexer's mirrors):
 *  - collection 28 `user_topic_signals` (restricted): group the user's uuid as a big-endian u128; key 33 bytes, user uuid,
 *    topic uuid, kind byte (vote 0, comment 1, debate 2, follow 3, interested 4, not_interested 5); score `decayed_count`
 *    (a non-finite one written as 0); value 36 bytes: `event_count` i32 LE, `last_event_at` µs i64 LE (i64 min for null),
 *    `top_object_id` uuid, `computed_at` µs i64 LE.
 *  - collection 23 `topic_cooccurrence` (open): group the topic's uuid; key topic uuid then neighbour uuid; score
 *    `similarity`; value `support` i32 LE. At most `spread_max_neighbours` (10) rows per topic.
 *  - collection 29 `feed_user_seen_daily` (restricted): group the user's uuid; key 36 bytes, user uuid, UTC day as days
 *    since the epoch (i32 BE), item uuid; score `last_seen_at` in epoch seconds; value 12 bytes: `impressions` i32 LE,
 *    `last_seen_at` µs i64 LE.
 *
 * The decay is applied at read time, as the SQL does: `weight × decayed_count × 0.5^(max(0, as_of − computed_at) /
 * (half_life_days × 86400))` for a kind whose `decays` is true, else `weight × decayed_count`; the weight and `decays` come
 * from `interest_signal_weights`, the half-life and `spread_fraction` from `interest_config` (geo-db's `config` Space
 * records, written by the geodb-feeder). Spreading, the per-topic sums, the top kind and the top-N follow the SQL's
 * order and tie-breaks: contribution DESC then kind ASC for the top kind, contribution DESC then via ASC for the
 * related source, weight DESC then topic ASC for the list. Uuids compare as their lowercase dashed text, which orders as
 * Postgres orders `uuid`.
 *
 * Floating point: Postgres sums in its own order and computes the power in its own library, so values agree to about
 * 1e-12 relative, not bit for bit. `close` is the comparison every caller uses.
 */

import type {GeoCardRow} from "./card"
import type {DerivedScanRow} from "./derived"

export const USER_TOPIC_SIGNALS_COLLECTION = 28
export const TOPIC_COOCCURRENCE_COLLECTION = 23
export const FEED_USER_SEEN_COLLECTION = 29

/** The mirror's kind bytes (ranking-indexer external_interest_mirror.rs `kind_tag`), by byte. */
export const KIND_BY_TAG = ["vote", "comment", "debate", "follow", "interested", "not_interested"] as const

/** The feeder's column pins (geodb-feeder/src/recordKinds.ts); a test holds them to that file. */
export const INTEREST_CONFIG_COLUMNS = {
	halfLifeDays: "1b50d435575f41aea81b5296b76eb480",
	spreadFraction: "1e771b65e07e4923bfa7c5b63620b349",
} as const
export const INTEREST_SIGNAL_WEIGHT_COLUMNS = {
	weight: "456ecbe7140a4d3bb399a9d28f4e9783",
	decays: "ac260e6563f740f59fcdb97a64599c64",
} as const

const I64_MIN = -(2n ** 63n)
const DAY_SECONDS = 86400
const DAY_MS = DAY_SECONDS * 1000

export class MalformedRow extends Error {}
export class AmbiguousConfig extends Error {}

/** 32 hex digits as the dashed lowercase uuid Postgres prints. */
export function dashedUuid(hex: string): string {
	const h = hex.toLowerCase()
	return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`
}

/** A uuid (dashed or not) as the 32 hex digits of the big-endian u128 the mirrors group by. */
export function groupHex(uuid: string): string {
	const h = uuid.replace(/-/g, "").toLowerCase()
	if (!/^[0-9a-f]{32}$/.test(h)) throw new Error("not a uuid")
	return h
}

const cmp = (a: string, b: string): number => (a < b ? -1 : a > b ? 1 : 0)

/** Equal within 1e-9 relative (absolute below 1): the sums' order and libm differ from Postgres', nothing else. */
export function close(a: number, b: number): boolean {
	return Math.abs(a - b) <= 1e-9 * Math.max(1, Math.abs(a), Math.abs(b))
}

// ---- config ---------------------------------------------------------------------------------------------------------

export type InterestConfig = {halfLifeDays: number; spreadFraction: number}
export type SignalWeight = {weight: number; decays: boolean}
export type SignalWeights = Map<string, SignalWeight>

function singleRaw(rows: GeoCardRow[], property: string, name: string): string {
	const values = new Set(rows.filter((r) => r.prop === property).map((r) => r.raw))
	if (values.size !== 1) throw new AmbiguousConfig(`${name} holds ${values.size} values`)
	return [...values][0] as string
}

function singleNumber(rows: GeoCardRow[], property: string, name: string): number {
	const raw = singleRaw(rows, property, name)
	const n = Number(raw)
	if (raw.trim() === "" || !Number.isFinite(n)) throw new AmbiguousConfig(`${name} is not a number`)
	return n
}

/** The `interest_config` record's two values the readers use, or null when the record has no statement. */
export function decodeInterestConfig(rows: GeoCardRow[]): InterestConfig | null {
	if (rows.length === 0) return null
	return {
		halfLifeDays: singleNumber(rows, INTEREST_CONFIG_COLUMNS.halfLifeDays, "half_life_days"),
		spreadFraction: singleNumber(rows, INTEREST_CONFIG_COLUMNS.spreadFraction, "spread_fraction"),
	}
}

/** One `interest_signal_weights` record, or null when it has no statement (the SQL's join then drops the kind). */
export function decodeSignalWeight(rows: GeoCardRow[]): SignalWeight | null {
	if (rows.length === 0) return null
	const decays = singleRaw(rows, INTEREST_SIGNAL_WEIGHT_COLUMNS.decays, "decays")
	if (decays !== "true" && decays !== "false") throw new AmbiguousConfig("decays is not a boolean")
	return {weight: singleNumber(rows, INTEREST_SIGNAL_WEIGHT_COLUMNS.weight, "weight"), decays: decays === "true"}
}

// ---- decoding -------------------------------------------------------------------------------------------------------

export type Signal = {
	topicId: string
	kind: string
	decayedCount: number
	eventCount: number
	/** Microseconds since the epoch, or null. */
	lastEventUs: number | null
	topObjectId: string
	/** Microseconds since the epoch. */
	computedUs: number
}

const i32le = (b: Buffer, at: number): number => b.readInt32LE(at)
const i64leUs = (b: Buffer, at: number): bigint => b.readBigInt64LE(at)

/** A collection 28 scan of `user`'s group. Rejects MalformedRow when a row is not the mirror's shape or not the user's. */
export function decodeSignals(userId: string, rows: DerivedScanRow[]): Signal[] {
	const user = groupHex(userId)
	return rows.map((r) => {
		if (r.keyHex.length !== 66 || r.valueHex.length !== 72) throw new MalformedRow("user_topic_signals row length")
		const key = r.keyHex.toLowerCase()
		if (key.slice(0, 32) !== user) throw new MalformedRow("user_topic_signals row of another group")
		const kind = KIND_BY_TAG[Number.parseInt(key.slice(64, 66), 16)]
		if (!kind) throw new MalformedRow("user_topic_signals kind byte")
		const v = Buffer.from(r.valueHex, "hex")
		const last = i64leUs(v, 4)
		return {
			topicId: dashedUuid(key.slice(32, 64)),
			kind,
			decayedCount: r.score,
			eventCount: i32le(v, 0),
			lastEventUs: last === I64_MIN ? null : Number(last),
			topObjectId: dashedUuid(v.subarray(12, 28).toString("hex")),
			computedUs: Number(i64leUs(v, 28)),
		}
	})
}

export type Neighbour = {neighbourId: string; similarity: number}

/** A collection 23 scan of `topic`'s group. */
export function decodeNeighbours(topicId: string, rows: DerivedScanRow[]): Neighbour[] {
	const topic = groupHex(topicId)
	return rows.map((r) => {
		const key = r.keyHex.toLowerCase()
		if (key.length !== 64 || r.valueHex.length !== 8) throw new MalformedRow("topic_cooccurrence row length")
		if (key.slice(0, 32) !== topic) throw new MalformedRow("topic_cooccurrence row of another group")
		return {neighbourId: dashedUuid(key.slice(32)), similarity: r.score}
	})
}

export type SeenRow = {day: number; itemId: string; impressions: number; lastSeenUs: number}

/** A collection 29 scan of `user`'s group. */
export function decodeSeen(userId: string, rows: DerivedScanRow[]): SeenRow[] {
	const user = groupHex(userId)
	return rows.map((r) => {
		const key = r.keyHex.toLowerCase()
		if (key.length !== 72 || r.valueHex.length !== 24) throw new MalformedRow("feed_user_seen_daily row length")
		if (key.slice(0, 32) !== user) throw new MalformedRow("feed_user_seen_daily row of another group")
		const k = Buffer.from(key, "hex")
		const v = Buffer.from(r.valueHex, "hex")
		return {
			day: k.readInt32BE(16),
			itemId: dashedUuid(key.slice(40)),
			impressions: i32le(v, 0),
			lastSeenUs: Number(i64leUs(v, 4)),
		}
	})
}

// ---- user_topic_contributions -----------------------------------------------------------------------------------------

export type Contribution = {
	topicId: string
	kind: string
	contribution: number
	eventCount: number | null
	lastEventUs: number | null
	topObjectId: string | null
	viaTopicId: string | null
}

/** The decayed, weighted contribution of each own signal, as `own_kinds` computes it. A kind with no weight is dropped (the join). */
export function ownContributions(
	signals: Signal[],
	config: InterestConfig,
	weights: SignalWeights,
	asOf: Date,
): Contribution[] {
	const asOfUs = asOf.getTime() * 1000
	const out: Contribution[] = []
	for (const s of signals) {
		const w = weights.get(s.kind)
		if (!w) continue
		const ageSeconds = Math.max(0, (asOfUs - s.computedUs) / 1e6)
		const decay = w.decays ? 0.5 ** (ageSeconds / (config.halfLifeDays * DAY_SECONDS)) : 1
		out.push({
			topicId: s.topicId,
			kind: s.kind,
			contribution: w.weight * s.decayedCount * decay,
			eventCount: s.eventCount,
			lastEventUs: s.lastEventUs,
			topObjectId: s.topObjectId,
			viaTopicId: null,
		})
	}
	return out
}

/** Each own topic's summed weight, as the SQL's `own` CTE. */
export function ownWeightsByTopic(own: Contribution[]): Map<string, number> {
	const sums = new Map<string, number>()
	for (const c of own) sums.set(c.topicId, (sums.get(c.topicId) ?? 0) + c.contribution)
	return sums
}

/** The own topics whose neighbours get a share: weight > 0, and only when `spread_fraction` > 0. */
export function spreadingTopics(own: Contribution[], config: InterestConfig): string[] {
	if (!(config.spreadFraction > 0)) return []
	return [...ownWeightsByTopic(own)]
		.filter(([, w]) => w > 0)
		.map(([t]) => t)
		.sort(cmp)
}

/**
 * The whole `user_topic_contributions` set (unordered, as the SQL's UNION ALL): the own rows, then for each own topic
 * of positive weight a `related` row per neighbour, `spread_fraction × weight × similarity / max(1, Σ similarity)`.
 * `neighbours` holds every spreading topic's collection 23 rows (a topic with none is fine; one missing from the map is
 * read as none, so the caller checks it scanned them all).
 */
export function userTopicContributions(
	signals: Signal[],
	config: InterestConfig,
	weights: SignalWeights,
	neighbours: Map<string, Neighbour[]>,
	asOf: Date,
): Contribution[] {
	const own = ownContributions(signals, config, weights, asOf)
	const ownWeights = ownWeightsByTopic(own)
	const related: Contribution[] = []
	for (const topicId of spreadingTopics(own, config)) {
		const rows = neighbours.get(topicId) ?? []
		const total = rows.reduce((sum, n) => sum + n.similarity, 0)
		const weight = ownWeights.get(topicId) as number
		for (const n of rows) {
			related.push({
				topicId: n.neighbourId,
				kind: "related",
				contribution: (config.spreadFraction * weight * n.similarity) / Math.max(1.0, total),
				eventCount: null,
				lastEventUs: null,
				topObjectId: null,
				viaTopicId: topicId,
			})
		}
	}
	return [...own, ...related]
}

// ---- user_topic_weights -----------------------------------------------------------------------------------------------

export type TopicWeightRow = {
	topicId: string
	weight: number
	ownWeight: number
	relatedWeight: number
	topKind: string
	topKindCount: number | null
	topObjectId: string | null
	relatedViaTopicId: string | null
}

/** `user_topic_weights` from the contributions: weight > 0, by weight DESC then topic ASC, at most `maxTopics`. */
export function userTopicWeights(contributions: Contribution[], maxTopics: number): TopicWeightRow[] {
	const rel = new Map<string, {sum: number; best: Contribution}>()
	const own = new Map<string, number>()
	for (const c of contributions) {
		if (c.kind === "related") {
			const r = rel.get(c.topicId)
			if (!r) rel.set(c.topicId, {sum: c.contribution, best: c})
			else {
				r.sum += c.contribution
				const b = r.best
				if (
					c.contribution > b.contribution ||
					(c.contribution === b.contribution && cmp(c.viaTopicId ?? "", b.viaTopicId ?? "") < 0)
				)
					r.best = c
			}
		} else own.set(c.topicId, (own.get(c.topicId) ?? 0) + c.contribution)
	}
	type Candidate = {kind: string; contribution: number; eventCount: number | null; topObjectId: string | null}
	const top = new Map<string, Candidate>()
	const offer = (topicId: string, c: Candidate) => {
		const t = top.get(topicId)
		if (!t || c.contribution > t.contribution || (c.contribution === t.contribution && cmp(c.kind, t.kind) < 0))
			top.set(topicId, c)
	}
	for (const c of contributions) if (c.kind !== "related") offer(c.topicId, c)
	for (const [topicId, r] of rel)
		offer(topicId, {kind: "related", contribution: r.sum, eventCount: null, topObjectId: r.best.viaTopicId})
	const rows: TopicWeightRow[] = []
	for (const [topicId, t] of top) {
		const ownWeight = own.get(topicId) ?? 0
		const relatedWeight = rel.get(topicId)?.sum ?? 0
		const weight = ownWeight + relatedWeight
		if (!(weight > 0)) continue
		rows.push({
			topicId,
			weight,
			ownWeight,
			relatedWeight,
			topKind: t.kind,
			topKindCount: t.eventCount,
			topObjectId: t.topObjectId,
			relatedViaTopicId: rel.get(topicId)?.best.viaTopicId ?? null,
		})
	}
	rows.sort((a, b) => b.weight - a.weight || cmp(a.topicId, b.topicId))
	return rows.slice(0, Math.max(0, maxTopics))
}

/** pairFit's `readOwnWeights` for one user: the top-N rows whose own weight is positive (the filter runs after the LIMIT). */
export function ownTopicWeights(rows: TopicWeightRow[]): {topicId: string; weight: number}[] {
	return rows.filter((r) => r.ownWeight > 0).map((r) => ({topicId: r.topicId, weight: r.ownWeight}))
}

/** `user_topic_interest_reasons`: one topic's contributions by contribution DESC, kind ASC, via ASC (nulls last). */
export function userTopicInterestReasons(contributions: Contribution[], topicId: string): Contribution[] {
	return contributions
		.filter((c) => c.topicId === topicId)
		.sort(
			(a, b) =>
				b.contribution - a.contribution ||
				cmp(a.kind, b.kind) ||
				(a.viaTopicId === b.viaTopicId
					? 0
					: a.viaTopicId === null
						? 1
						: b.viaTopicId === null
							? -1
							: cmp(a.viaTopicId, b.viaTopicId)),
		)
}

// ---- feed_user_seen -----------------------------------------------------------------------------------------------------

export type SeenCount = {itemId: string; seenCount: number; lastSeenUs: number}

/** The UTC day of `since` as days since the epoch: `(since AT TIME ZONE 'UTC')::date`. */
export const utcDay = (at: Date): number => Math.floor(at.getTime() / DAY_MS)

/**
 * `feed_user_seen(user, since)`: per item, the impressions of the rows from `since`'s UTC day on, kept when the item's
 * latest `last_seen_at` is at or after `since`, latest first. Ties on the latest are not ordered by the SQL; here by item.
 */
export function feedUserSeen(rows: SeenRow[], since: Date): SeenCount[] {
	const fromDay = utcDay(since)
	const sinceUs = since.getTime() * 1000
	const byItem = new Map<string, {seen: number; last: number}>()
	for (const r of rows) {
		if (r.day < fromDay) continue
		const e = byItem.get(r.itemId)
		if (!e) byItem.set(r.itemId, {seen: r.impressions, last: r.lastSeenUs})
		else {
			e.seen += r.impressions
			e.last = Math.max(e.last, r.lastSeenUs)
		}
	}
	return [...byItem]
		.filter(([, e]) => e.last >= sinceUs)
		.map(([itemId, e]) => ({itemId, seenCount: e.seen, lastSeenUs: e.last}))
		.sort((a, b) => b.lastSeenUs - a.lastSeenUs || cmp(a.itemId, b.itemId))
}

/** `readSeen`'s counts map from the twin's rows: items with a positive count. */
export function seenCounts(rows: SeenCount[]): Map<string, number> {
	const counts = new Map<string, number>()
	for (const r of rows) if (r.seenCount > 0) counts.set(r.itemId, r.seenCount)
	return counts
}

/**
 * Whether one page of a collection 29 scan holds every row `feed_user_seen(user, since)` reads. The scan walks
 * `last_seen_at` DESC, and a row's `last_seen_at` falls on its own UTC day, so once a page reaches a row older than the
 * start of `since`'s day every later row is from an earlier day. A full page that has not got there is truncated.
 */
export function seenPageComplete(scan: {rows: DerivedScanRow[]; more: boolean}, since: Date): boolean {
	if (!scan.more) return true
	const last = scan.rows[scan.rows.length - 1]
	return last !== undefined && last.score < utcDay(since) * DAY_SECONDS
}
