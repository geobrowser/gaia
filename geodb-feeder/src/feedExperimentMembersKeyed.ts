/**
 * `personalization.feed_experiment_members` as natural-key records (geo-db plan 10.12 K1-K5; the `config` family),
 * behind GEODB_FEED_EXPERIMENT_MEMBERS, which takes effect only with GEODB_RECORD_KEYS
 * (feedExperimentMembersKeyedEnabled). With either off nothing here runs.
 *
 * A mirror, not a move, in the shape of feedExperimentsKeyed.ts. The one reader is the plpgsql
 * `personalization.feed_experiment_for_user` (api/drizzle/0102:184-193), called by `readExperiment`
 * (api/src/forYou/queries.ts:236); the api's shadow (api/src/forYou/feedExperimentShadow.ts) compares a member's
 * served `interleaved` with these records. Only migrations and operator SQL write the table, so there is no
 * consumer: the backfill Job (`feed-experiment-members-backfill-main.ts`) is the only writer; rerun it after an edit.
 *
 * One record per (experiment_id, user_id): entity `recordEntityId(feedExperimentMember, [text(experiment), id(user)])`
 * (the user as its 16-byte uuid), key columns `experiment_id` and `user_id` (lowercase dashed text), and one mutable
 * column `interleaved` (boolean, a `put`; a flip closes the old value and A -> B -> A ends on A). `added_at` is not
 * mirrored. A membership deleted from Postgres is not cleared in geo-db; the gate reports it.
 */
import {applyKeyedRecord, backfillKeyedRows, type KeyedBackfillResult, type KeyedOutcome, type KeyedRecordDeps, keyColumn} from "./keyedRecord.js"
import {idPart, intPart, recordEntityId, recordStatementId, textPart} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {putIntent, zigString} from "./translate.js"

const KIND = RECORD_KINDS.feedExperimentMember.id
const FAMILY = RECORD_KINDS.feedExperimentMember.family
const COL = RECORD_COLUMNS.feedExperimentMember

export type FeedExperimentMember = {experiment: string; user: string; interleaved: boolean}

const UUID = /^[0-9a-f]{8}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{12}$/i
const dashed = (u: string): string => {
	const c = u.replaceAll("-", "").toLowerCase()
	return `${c.slice(0, 8)}-${c.slice(8, 12)}-${c.slice(12, 16)}-${c.slice(16, 20)}-${c.slice(20)}`
}

/** One membership's record entity id. `user` is a uuid, dashed or not. */
export const feedExperimentMemberRecordId = (experiment: string, user: string): string =>
	recordEntityId(KIND, [textPart(experiment), idPart(user)])

export function feedExperimentMemberIntents(m: FeedExperimentMember): string[] {
	const record = feedExperimentMemberRecordId(m.experiment, m.user)
	const user = dashed(m.user)
	const key = (property: string, value: string) =>
		keyColumn(
			recordStatementId(KIND, [idPart(record), idPart(property), textPart(value)]),
			record,
			property,
			`.{ .text = ${zigString(value)} }`,
		)
	return [
		`.{ .create_entity = "${record}" }`,
		...key(COL.experiment, m.experiment),
		...key(COL.user, user),
		putIntent({
			id: recordStatementId(KIND, [idPart(record), idPart(COL.interleaved), intPart(m.interleaved ? 1 : 0)]),
			entity: record,
			property: COL.interleaved,
			value: `.{ .boolean = ${m.interleaved} }`,
		}),
	]
}

export type KeyedFeedExperimentMembersDeps = KeyedRecordDeps

export const applyFeedExperimentMember = (
	deps: KeyedFeedExperimentMembersDeps,
	m: FeedExperimentMember,
	meta: {partition: number; offset: string},
): Promise<KeyedOutcome> =>
	applyKeyedRecord(deps, FAMILY, feedExperimentMemberIntents(m), {...meta, space: "feed_experiment_members"})

/** Whether the keyed writer runs: GEODB_FEED_EXPERIMENT_MEMBERS only takes effect with GEODB_RECORD_KEYS. */
export const feedExperimentMembersKeyedEnabled = (c: {recordKeys: boolean; feedExperimentMembers: {enabled: boolean}}): boolean =>
	c.feedExperimentMembers.enabled && c.recordKeys

/** The backfill gate's offsets directory under DATA_DIR (there is no consumer, so it stays empty). */
export const feedExperimentMembersOffsetsDir = (dataDir: string) => `${dataDir}/feeder/feed-experiment-members-keyed`

/** A row as the backfill reads it (pgSource.readFeedExperimentMemberRows): `user_id` as text. */
export type FeedExperimentMemberRow = {experiment_id: string; user_id: string; interleaved: boolean}

/** A Postgres row as a membership. Throws on what the table could not have held. */
export function feedExperimentMemberFromRow(r: FeedExperimentMemberRow): FeedExperimentMember {
	if (typeof r.experiment_id !== "string" || r.experiment_id === "")
		throw new Error(`feed_experiment_members: experiment_id ${JSON.stringify(r.experiment_id)}`)
	if (typeof r.user_id !== "string" || !UUID.test(r.user_id))
		throw new Error(`feed_experiment_members ${r.experiment_id}: user_id ${JSON.stringify(r.user_id)}`)
	if (typeof r.interleaved !== "boolean")
		throw new Error(`feed_experiment_members ${r.experiment_id}: interleaved ${JSON.stringify(r.interleaved)}`)
	return {experiment: r.experiment_id, user: dashed(r.user_id), interleaved: r.interleaved}
}

/** Write every membership. Rerunnable. A row the table could not hold is dead-lettered, not invented. */
export const backfillFeedExperimentMembers = (
	deps: KeyedFeedExperimentMembersDeps,
	rows: AsyncIterable<FeedExperimentMemberRow> | Iterable<FeedExperimentMemberRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{
			topic: "feed_experiment_members",
			fromRow: feedExperimentMemberFromRow,
			apply: (d, m) => applyFeedExperimentMember(d, m, {partition: -1, offset: "backfill"}),
		},
		progress,
	)
