/**
 * `entity_ranking_config` as one natural-key record (geo-db plan 10.12 K1-K5; plan 10.11 GAP-7, the `config` family;
 * inventory 11 §3.4 / §6), behind GEODB_ENTITY_RANKING_CONFIG, which takes effect only with GEODB_RECORD_KEYS
 * (entityRankingConfigKeyedEnabled). With either off nothing here runs.
 *
 * This is a mirror, not a move. Postgres stays the source: the ranking plpgsql functions (`SELECT * INTO cfg FROM
 * entity_ranking_config WHERE id`, migrations 0073..0118), the feed-composition samplers and `readSettings`
 * (api/src/forYou/queries.ts, tau_seconds) read the table, and the api's shadow (api/src/forYou/entityRankingConfigShadow.ts)
 * compares the tau_seconds the For you route served with this record. There is no event stream to consume (nothing writes
 * the table but migrations and operator SQL), so the only writer is the backfill Job,
 * `entity-ranking-config-backfill-main.ts`: rerun it after an operator edits the row. It is rerunnable and writes the same
 * ids again.
 *
 * One record, in the `config` system Space, the same shape as forYouConfigKeyed.ts:
 * - record entity: `recordEntityId(entityRankingConfig, [int(1)])`, the table's key (`id` = true, one row);
 * - key column (RECORD_COLUMNS.entityRankingConfig.id): `add` + `set_active(true)`, the number 1;
 * - mutable columns (ENTITY_RANKING_COLUMNS): `put` of each, a decimal for numeric columns and a number for the five
 *   integer ones, under `recordStatementId(entityRankingConfig, [record, property, value])`, so a changed value closes the
 *   old one and A -> B -> A ends on A. `updated_at` is not mirrored: it is bookkeeping, not a tunable.
 */
import {
	applyKeyedRecord,
	backfillKeyedRows,
	type KeyedBackfillResult,
	type KeyedOutcome,
	type KeyedRecordDeps,
	keyColumn,
} from "./keyedRecord.js"
import {idPart, intPart, recordEntityId, recordStatementId, textPart} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {floatToDecimal, putIntent} from "./translate.js"

const KIND = RECORD_KINDS.entityRankingConfig.id
const FAMILY = RECORD_KINDS.entityRankingConfig.family
const COL = RECORD_COLUMNS.entityRankingConfig

export type EntityRankingKey = Exclude<keyof typeof COL, "id">

/** Every mirrored column: the Postgres name, the key in RECORD_COLUMNS, and whether it is an `integer` (else `numeric`). */
export const ENTITY_RANKING_COLUMNS: readonly {pg: string; key: EntityRankingKey; int: boolean}[] = [
	{pg: "tau_seconds", key: "tauSeconds", int: false},
	{pg: "wilson_z", key: "wilsonZ", int: false},
	{pg: "prior_positive", key: "priorPositive", int: false},
	{pg: "prior_negative", key: "priorNegative", int: false},
	{pg: "quality_floor", key: "qualityFloor", int: false},
	{pg: "intrinsic_cap", key: "intrinsicCap", int: false},
	{pg: "intrinsic_property_target", key: "intrinsicPropertyTarget", int: true},
	{pg: "intrinsic_relation_target", key: "intrinsicRelationTarget", int: true},
	{pg: "participation_weight", key: "participationWeight", int: false},
	{pg: "participation_cap", key: "participationCap", int: false},
	{pg: "comment_weight", key: "commentWeight", int: false},
	{pg: "comment_cap", key: "commentCap", int: false},
	{pg: "topic_interested_weight", key: "topicInterestedWeight", int: false},
	{pg: "topic_debate_weight", key: "topicDebateWeight", int: false},
	{pg: "topic_cap", key: "topicCap", int: false},
	{pg: "watch_window_days", key: "watchWindowDays", int: true},
	{pg: "watch_prior_impressions", key: "watchPriorImpressions", int: false},
	{pg: "watch_reference_seconds", key: "watchReferenceSeconds", int: false},
	{pg: "watch_ratio_weight", key: "watchRatioWeight", int: false},
	{pg: "watch_completed_weight", key: "watchCompletedWeight", int: false},
	{pg: "watch_plays_weight", key: "watchPlaysWeight", int: false},
	{pg: "watch_claim_votes_weight", key: "watchClaimVotesWeight", int: false},
	{pg: "watch_cap", key: "watchCap", int: false},
	{pg: "watch_fresh_boost", key: "watchFreshBoost", int: false},
	{pg: "watch_fresh_impressions", key: "watchFreshImpressions", int: false},
	{pg: "trending_boost", key: "trendingBoost", int: false},
	{pg: "trending_topic_boost", key: "trendingTopicBoost", int: false},
	{pg: "trending_day_factor", key: "trendingDayFactor", int: false},
	{pg: "trending_debate_factor", key: "trendingDebateFactor", int: false},
	{pg: "trending_debate_window_days", key: "trendingDebateWindowDays", int: true},
	{pg: "trending_horizon_days", key: "trendingHorizonDays", int: true},
]

/** The row's values: integers as bigint, numerics as numbers. */
export type EntityRankingConfig = Record<EntityRankingKey, number | bigint>

/** The one record's entity id. */
export const entityRankingConfigRecordId = (): string => recordEntityId(KIND, [intPart(1)])

/** The intents for the config record. Pure, and depends on the values alone. */
export function entityRankingConfigIntents(c: EntityRankingConfig): string[] {
	const record = entityRankingConfigRecordId()
	const column = ({pg, key, int}: (typeof ENTITY_RANKING_COLUMNS)[number]): string => {
		const v = c[key]
		const property = COL[key]
		if (int) {
			if (typeof v !== "bigint") throw new Error(`entity_ranking_config: ${pg} ${v}`)
			return putIntent({
				id: recordStatementId(KIND, [idPart(record), idPart(property), intPart(BigInt.asUintN(64, v))]),
				entity: record,
				property,
				value: `.{ .number = ${v} }`,
			})
		}
		const d = typeof v === "number" ? floatToDecimal(v) : null
		if (!d) throw new Error(`entity_ranking_config: ${pg} ${v}`)
		const text = `${d[0]},${d[1]}`
		return putIntent({
			id: recordStatementId(KIND, [idPart(record), idPart(property), textPart(text)]),
			entity: record,
			property,
			value: `.{ .decimal = "${text}" }`,
		})
	}
	return [
		`.{ .create_entity = "${record}" }`,
		...keyColumn(
			recordStatementId(KIND, [idPart(record), idPart(COL.id), intPart(1)]),
			record,
			COL.id,
			".{ .number = 1 }",
		),
		...ENTITY_RANKING_COLUMNS.map(column),
	]
}

export type KeyedEntityRankingConfigDeps = KeyedRecordDeps

export const applyEntityRankingConfig = (
	deps: KeyedEntityRankingConfigDeps,
	c: EntityRankingConfig,
	meta: {partition: number; offset: string},
): Promise<KeyedOutcome> =>
	applyKeyedRecord(deps, FAMILY, entityRankingConfigIntents(c), {...meta, space: "entity_ranking_config"})

/** Whether the keyed writer runs: GEODB_ENTITY_RANKING_CONFIG only takes effect with GEODB_RECORD_KEYS. */
export const entityRankingConfigKeyedEnabled = (c: {
	recordKeys: boolean
	entityRankingConfig: {enabled: boolean}
}): boolean => c.entityRankingConfig.enabled && c.recordKeys

/** The backfill gate's offsets directory under DATA_DIR (there is no consumer, so it stays empty). */
export const entityRankingConfigOffsetsDir = (dataDir: string) => `${dataDir}/feeder/entity-ranking-config-keyed`

/** The row as the backfill reads it (pgSource.readEntityRankingConfigRows): every column as text, so nothing rounds on the way. */
export type EntityRankingConfigRow = Record<string, string>

/** A Postgres row as a config. Throws on a value the table's checks would have refused. */
export function entityRankingConfigFromRow(r: EntityRankingConfigRow): EntityRankingConfig {
	const out: Partial<EntityRankingConfig> = {}
	for (const {pg, key, int} of ENTITY_RANKING_COLUMNS) {
		const v = r[pg]
		if (typeof v !== "string") throw new Error(`entity_ranking_config: ${pg} missing`)
		if (int) {
			if (!/^-?\d+$/.test(v)) throw new Error(`entity_ranking_config: ${pg} ${JSON.stringify(v)}`)
			out[key] = BigInt(v)
		} else {
			const n = Number(v)
			if (v.trim() === "" || !Number.isFinite(n)) throw new Error(`entity_ranking_config: ${pg} ${JSON.stringify(v)}`)
			out[key] = n
		}
	}
	return out as EntityRankingConfig
}

/** Write the table's row as the config record. Rerunnable. A row the table could not hold is dead-lettered, not invented. */
export const backfillEntityRankingConfig = (
	deps: KeyedEntityRankingConfigDeps,
	rows: AsyncIterable<EntityRankingConfigRow> | Iterable<EntityRankingConfigRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{
			topic: "entity_ranking_config",
			fromRow: entityRankingConfigFromRow,
			apply: (d, c) => applyEntityRankingConfig(d, c, {partition: -1, offset: "backfill"}),
		},
		progress,
	)
