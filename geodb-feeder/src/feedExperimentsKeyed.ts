/**
 * `personalization.feed_experiments` as natural-key records (geo-db plan 10.12 K1-K5; plan 10.11 GAP-7, the `config`
 * family; inventory 11 §3.4), behind GEODB_FEED_EXPERIMENTS, which takes effect only with GEODB_RECORD_KEYS
 * (feedExperimentsKeyedEnabled). With either off nothing here runs.
 *
 * This is a mirror, not a move. Postgres stays the source: the one reader is the plpgsql
 * `personalization.feed_experiment_for_user` (api/drizzle/0102:184), called by `readExperiment`
 * (api/src/forYou/queries.ts:236), and the api's shadow (api/src/forYou/feedExperimentShadow.ts) compares the served
 * experiment with these records. Nothing writes the table but migrations and operator SQL (an experiment is a row change,
 * 0102:147), so there is no event stream to consume: the only writer is the backfill Job
 * (`feed-experiments-backfill-main.ts`), rerun it after an operator edits a row. It is rerunnable and writes the same ids
 * again.
 *
 * One record per experiment `id`, in the `config` system Space, the shape of interestConfigKeyed.ts:
 * - record entity: `recordEntityId(feedExperiment, [text(id)])`;
 * - key column `id` (text);
 * - mutable columns, each a `put` under `recordStatementId(feedExperiment, [record, property, value])`, so a changed value
 *   closes the old one and A -> B -> A ends on A: `active` (boolean), `arm_a`, `arm_b`, `salt` (text), `share` (decimal),
 *   `note` (text, written only when not null).
 * `created_at` is not mirrored (bookkeeping). `salt` is: the shadow needs it to recompute the hash assignment, and it is
 * a bucketing salt, not a secret. `feed_experiment_members` (explicit membership) is not mirrored; the shadow compares the
 * hash assignment only for a user the table does not hold as a member.
 * A note changed to null, or an experiment deleted from Postgres, is not cleared in geo-db; the gate reports it.
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
import {floatToDecimal, putIntent, zigString} from "./translate.js"

const KIND = RECORD_KINDS.feedExperiment.id
const FAMILY = RECORD_KINDS.feedExperiment.family
const COL = RECORD_COLUMNS.feedExperiment

export type FeedExperiment = {
	id: string
	active: boolean
	armA: string
	armB: string
	share: number
	salt: string
	note: string | null
}

/** One experiment's record entity id. */
export const feedExperimentRecordId = (id: string): string => recordEntityId(KIND, [textPart(id)])

/** The intents for one experiment. Pure, and depends on the values alone. */
export function feedExperimentIntents(e: FeedExperiment): string[] {
	const record = feedExperimentRecordId(e.id)
	const put = (property: string, part: ReturnType<typeof textPart>, value: string) =>
		putIntent({id: recordStatementId(KIND, [idPart(record), idPart(property), part]), entity: record, property, value})
	const text = (property: string, v: string) => put(property, textPart(v), `.{ .text = ${zigString(v)} }`)
	const d = floatToDecimal(e.share)
	if (!d) throw new Error(`feed_experiments ${e.id}: share ${e.share}`)
	const share = `${d[0]},${d[1]}`
	return [
		`.{ .create_entity = "${record}" }`,
		...keyColumn(
			recordStatementId(KIND, [idPart(record), idPart(COL.id), textPart(e.id)]),
			record,
			COL.id,
			`.{ .text = ${zigString(e.id)} }`,
		),
		put(COL.active, intPart(e.active ? 1 : 0), `.{ .boolean = ${e.active} }`),
		text(COL.armA, e.armA),
		text(COL.armB, e.armB),
		put(COL.share, textPart(share), `.{ .decimal = "${share}" }`),
		text(COL.salt, e.salt),
		...(e.note === null ? [] : [text(COL.note, e.note)]),
	]
}

export type KeyedFeedExperimentsDeps = KeyedRecordDeps

export const applyFeedExperiment = (
	deps: KeyedFeedExperimentsDeps,
	e: FeedExperiment,
	meta: {partition: number; offset: string},
): Promise<KeyedOutcome> => applyKeyedRecord(deps, FAMILY, feedExperimentIntents(e), {...meta, space: "feed_experiments"})

/** Whether the keyed writer runs: GEODB_FEED_EXPERIMENTS only takes effect with GEODB_RECORD_KEYS. */
export const feedExperimentsKeyedEnabled = (c: {recordKeys: boolean; feedExperiments: {enabled: boolean}}): boolean =>
	c.feedExperiments.enabled && c.recordKeys

/** The backfill gate's offsets directory under DATA_DIR (there is no consumer, so it stays empty). */
export const feedExperimentsOffsetsDir = (dataDir: string) => `${dataDir}/feeder/feed-experiments-keyed`

/** A row as the backfill reads it (pgSource.readFeedExperimentRows): `share` as text. */
export type FeedExperimentRow = {
	id: string
	active: boolean
	arm_a: string
	arm_b: string
	share: string
	salt: string
	note: string | null
}

/** A Postgres row as an experiment. Throws on what the table's checks would have refused. */
export function feedExperimentFromRow(r: FeedExperimentRow): FeedExperiment {
	const str = (k: keyof FeedExperimentRow): string => {
		const v = r[k]
		if (typeof v !== "string") throw new Error(`feed_experiments: ${k} ${JSON.stringify(v)}`)
		return v
	}
	const id = str("id")
	if (id === "") throw new Error("feed_experiments: id is empty")
	const arm = (k: "arm_a" | "arm_b") => {
		const v = str(k)
		if (v !== "best" && v !== "for-you") throw new Error(`feed_experiments ${id}: ${k} ${JSON.stringify(v)}`)
		return v
	}
	const shareText = str("share")
	const share = Number(shareText)
	if (shareText.trim() === "" || !Number.isFinite(share) || share < 0 || share > 1)
		throw new Error(`feed_experiments ${id}: share ${JSON.stringify(shareText)}`)
	if (typeof r.active !== "boolean") throw new Error(`feed_experiments ${id}: active ${JSON.stringify(r.active)}`)
	if (r.note !== null && typeof r.note !== "string") throw new Error(`feed_experiments ${id}: note ${JSON.stringify(r.note)}`)
	return {id, active: r.active, armA: arm("arm_a"), armB: arm("arm_b"), share, salt: str("salt"), note: r.note}
}

/** Write every experiment. Rerunnable. A row the table could not hold is dead-lettered, not invented. */
export const backfillFeedExperiments = (
	deps: KeyedFeedExperimentsDeps,
	rows: AsyncIterable<FeedExperimentRow> | Iterable<FeedExperimentRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{
			topic: "feed_experiments",
			fromRow: feedExperimentFromRow,
			apply: (d, e) => applyFeedExperiment(d, e, {partition: -1, offset: "backfill"}),
		},
		progress,
	)
