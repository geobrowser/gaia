/**
 * `personalization.for_you_config` as one natural-key record (geo-db plan 10.12 K1-K5; plan 10.11 GAP-7, the `config`
 * family; inventory 11 §3.4), behind GEODB_FOR_YOU_CONFIG, which takes effect only with GEODB_RECORD_KEYS
 * (forYouConfigKeyedEnabled). With either off nothing here runs.
 *
 * This is a mirror, not a move. Postgres stays the source: `readSettings` (api/src/forYou/queries.ts) and the plpgsql
 * `user_topic_weights(.., max_topics)` read the table, and the api's for-you config shadow (api/src/forYou/configShadow.ts)
 * compares what Postgres served with this record. There is no event stream to consume (nothing writes the table but
 * migrations and operator SQL), so the only writer is the backfill Job, `for-you-config-backfill-main.ts`: rerun it after
 * an operator edits the row. It is rerunnable and writes the same ids again.
 *
 * One record, in the `config` system Space:
 * - record entity: `recordEntityId(forYouConfig, [int(1)])`, the table's key (`id` = true, one row);
 * - key column (RECORD_COLUMNS.forYouConfig.id): `add` + `set_active(true)`, the number 1;
 * - mutable columns: `put` of max_boost_days, interest_half_saturation and exploration_share (decimals) and max_topics
 *   (a number), each under `recordStatementId(forYouConfig, [record, property, value])`, so a changed value closes the
 *   old one and A -> B -> A ends on A.
 *
 * The revision counter (`feed_config_revision`) is not written: it is derived (inventory §6).
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

const KIND = RECORD_KINDS.forYouConfig.id
const FAMILY = RECORD_KINDS.forYouConfig.family
const COL = RECORD_COLUMNS.forYouConfig

export type ForYouConfig = {
	maxBoostDays: number
	interestHalfSaturation: number
	explorationShare: number
	maxTopics: bigint
}

/** The one record's entity id. */
export const forYouConfigRecordId = (): string => recordEntityId(KIND, [intPart(1)])

/** The intents for the config record. Pure, and depends on the values alone. */
export function forYouConfigIntents(c: ForYouConfig): string[] {
	const record = forYouConfigRecordId()
	const decimal = (property: string, v: number): string => {
		const d = floatToDecimal(v)
		if (!d) throw new Error(`for_you_config: ${property} ${v}`)
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
		decimal(COL.maxBoostDays, c.maxBoostDays),
		decimal(COL.interestHalfSaturation, c.interestHalfSaturation),
		decimal(COL.explorationShare, c.explorationShare),
		putIntent({
			id: recordStatementId(KIND, [
				idPart(record),
				idPart(COL.maxTopics),
				intPart(BigInt.asUintN(64, c.maxTopics)),
			]),
			entity: record,
			property: COL.maxTopics,
			value: `.{ .number = ${c.maxTopics} }`,
		}),
	]
}

export type KeyedForYouConfigDeps = KeyedRecordDeps

export const applyForYouConfig = (
	deps: KeyedForYouConfigDeps,
	c: ForYouConfig,
	meta: {partition: number; offset: string},
): Promise<KeyedOutcome> => applyKeyedRecord(deps, FAMILY, forYouConfigIntents(c), {...meta, space: "for_you_config"})

/** Whether the keyed writer runs: GEODB_FOR_YOU_CONFIG only takes effect with GEODB_RECORD_KEYS. */
export const forYouConfigKeyedEnabled = (c: {recordKeys: boolean; forYouConfig: {enabled: boolean}}): boolean =>
	c.forYouConfig.enabled && c.recordKeys

/** The backfill gate's offsets directory under DATA_DIR (there is no consumer, so it stays empty). */
export const forYouConfigOffsetsDir = (dataDir: string) => `${dataDir}/feeder/for-you-config-keyed`

/** The row as the backfill reads it (pgSource.readForYouConfigRows): numbers as text, so nothing rounds on the way. */
export type ForYouConfigRow = {
	max_boost_days: string
	interest_half_saturation: string
	exploration_share: string
	max_topics: string
}

const num = (k: string, v: string): number => {
	const n = Number(v)
	if (v.trim() === "" || !Number.isFinite(n)) throw new Error(`for_you_config: ${k} ${JSON.stringify(v)}`)
	return n
}

/** A Postgres row as a config. Throws on a value the table's checks would have refused. */
export function forYouConfigFromRow(r: ForYouConfigRow): ForYouConfig {
	if (!/^\d+$/.test(r.max_topics)) throw new Error(`for_you_config: max_topics ${JSON.stringify(r.max_topics)}`)
	return {
		maxBoostDays: num("max_boost_days", r.max_boost_days),
		interestHalfSaturation: num("interest_half_saturation", r.interest_half_saturation),
		explorationShare: num("exploration_share", r.exploration_share),
		maxTopics: BigInt(r.max_topics),
	}
}

/** Write the table's row as the config record. Rerunnable. A row the table could not hold is dead-lettered, not invented. */
export const backfillForYouConfig = (
	deps: KeyedForYouConfigDeps,
	rows: AsyncIterable<ForYouConfigRow> | Iterable<ForYouConfigRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{
			topic: "personalization.for_you_config",
			fromRow: forYouConfigFromRow,
			apply: (d, c) => applyForYouConfig(d, c, {partition: -1, offset: "backfill"}),
		},
		progress,
	)
