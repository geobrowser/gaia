/**
 * Space voting settings as natural-key records (geo-db plan 10.12 K1-K5; plan 10.11 GAP-7, the `governance` family),
 * behind GEODB_SPACE_VOTING_SETTINGS, which takes effect only with GEODB_RECORD_KEYS (spaceVotingSettingsKeyedEnabled).
 * With either off nothing here runs. A flag of its own: the backfill must run before the consumer first starts.
 *
 * Source: `<env>.space.governance`, messages whose `event-type` header is `VOTING_SETTINGS_UPDATED`
 * (HermesVotingSettingsUpdated, hermes-schema/proto/governance.proto), the events kg-indexer folds into gaia's
 * `space_voting_settings` (handlers/governance.rs handle_voting_settings_updated; storage.rs
 * upsert_space_voting_settings, `ON CONFLICT (space_id) DO UPDATE` overwriting every column). Every other event type is
 * ignored. One record per Space, in the `governance` system Space (RECORD_KINDS.spaceVotingSettings is pinned to that
 * family):
 * - record entity: `recordEntityId(spaceVotingSettings, [id(space)])`, the raw 16-byte gaia id, the table's key;
 * - key column (RECORD_COLUMNS.spaceVotingSettings): `add` + `set_active(true)`, a reference to the Space's entity;
 * - mutable columns: `put` of partial, universal, flat, quorum, duration and grace (numbers), disable_fast_path
 *   (boolean), updated_at (instant) and updated_at_block (number), each under
 *   `recordStatementId(spaceVotingSettings, [record, property, int(value)])`. A put closes the cell's other active
 *   statements, so a changed setting replaces the old one, as `ON CONFLICT DO UPDATE` does, and including the value in
 *   the id makes A -> B -> A end on A and a replayed event a no-op.
 *
 * Every event's intents are fixed by the event alone: no read before write, no journal. hermes keys the event by its
 * Space, so one key's events arrive in order on one partition and a replay converges on the last event per key, as
 * Postgres does. Values are `as i64` (the cast kg-indexer makes) and keyed as their u64 bit pattern.
 *
 * Accepted and rejected where kg-indexer accepts and rejects: a `space_id` that is not 16 bytes is dead-lettered
 * (Uuid::from_slice errors); missing metadata is written as time 0, block 0 (`unwrap_or((0, 0))`). A wire type protobuf
 * would not decode is a decode failure here (thrown, dead-lettered), never read as a default.
 *
 * Not mirrored, in either direction: kg-indexer drops the WHOLE block when any write in it fails permanently (this
 * table's `space_voting_settings_space_id_spaces_id_fk` is an FK to `spaces`), so geo-db can hold settings Postgres
 * lacks, or newer ones. Treat any difference between `space_voting_settings` and these records as expected and
 * investigate it, never as a bug in either (spaceVotingSettingsGate.ts reports geo-db-only keys, not as a failure).
 *
 * Related: an UpdateVotingSettings proposal ACTION is a `proposal_actions` row (proposalActionsKeyed.ts); the setting
 * itself changes only when the on-chain VotingSettingsUpdated event arrives, which is this consumer's input.
 */
import {parseFields} from "./hermes.js"
import {cid} from "./ids.js"
import {
	applyKeyedRecord,
	backfillKeyedRows,
	deadLetterDecode,
	type KeyedBackfillResult,
	type KeyedOutcome,
	type KeyedRecordDeps,
	keyColumn,
} from "./keyedRecord.js"
import {idPart, intPart, recordEntityId, recordStatementId} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {putIntent} from "./translate.js"

const KIND = RECORD_KINDS.spaceVotingSettings.id
const FAMILY = RECORD_KINDS.spaceVotingSettings.family
const COL = RECORD_COLUMNS.spaceVotingSettings

/** The `event-type` header value kg-indexer decodes as HermesVotingSettingsUpdated. */
export const VOTING_SETTINGS_UPDATED = "VOTING_SETTINGS_UPDATED"

export type SpaceVotingSettings = {
	/** 32 hex digits. */
	space: string
	partial: bigint
	universal: bigint
	flat: bigint
	quorum: bigint
	duration: bigint
	disableFastPath: boolean
	grace: bigint
	/** Block time in whole seconds (i64, as Postgres's text holds it). */
	updatedAtSeconds: bigint
	updatedAtBlock: bigint
}

const i64 = (v: bigint) => BigInt.asIntN(64, v)

/** Decode a HermesVotingSettingsUpdated with kg-indexer's handle_voting_settings_updated rules. Throws where it errors. */
export function parseVotingSettingsUpdated(b: Uint8Array): SpaceVotingSettings {
	const f = new Map<number, bigint | Uint8Array>()
	for (const [n, v] of parseFields(b)) f.set(n, v)
	const num = (n: number, what: string): bigint => {
		const v = f.get(n)
		if (v === undefined) return 0n
		if (typeof v !== "bigint") throw new Error(`voting settings: ${what} has an invalid wire type`)
		return v
	}
	const id = f.get(1) ?? new Uint8Array(0)
	if (!(id instanceof Uint8Array) || id.length !== 16) throw new Error("voting settings: space_id is not 16 bytes")
	let createdAt = 0n
	let block = 0n
	const meta = f.get(9)
	if (meta !== undefined && !(meta instanceof Uint8Array)) throw new Error("voting settings: invalid metadata wire type")
	if (meta !== undefined) {
		for (const [n, v] of parseFields(meta)) {
			if (n === 1 || n === 3) {
				if (typeof v !== "bigint") throw new Error(`voting settings: invalid metadata field ${n} wire type`)
				if (n === 1) createdAt = v
				else block = v
			}
		}
	}
	return {
		space: Buffer.from(id).toString("hex"),
		partial: i64(num(2, "partial")),
		universal: i64(num(3, "universal")),
		flat: i64(num(4, "flat")),
		quorum: i64(num(5, "quorum")),
		duration: i64(num(6, "duration")),
		disableFastPath: num(7, "disable_fast_path") !== 0n,
		grace: i64(num(8, "grace")),
		updatedAtSeconds: i64(createdAt),
		updatedAtBlock: i64(block),
	}
}

/** The record entity of one Space's settings. */
export const spaceVotingSettingsRecordId = (space: string): string => recordEntityId(KIND, [idPart(space)])

// A signed value's statement-id key part is its u64 bit pattern (record keys hold unsigned ints).
const valueStatementId = (record: string, property: string, value: bigint | number) =>
	recordStatementId(KIND, [idPart(record), idPart(property), intPart(BigInt.asUintN(64, BigInt(value)))])

/** The intents for one Space's settings. Pure, and depends on the settings alone. */
export function spaceVotingSettingsIntents(s: SpaceVotingSettings): string[] {
	const record = spaceVotingSettingsRecordId(s.space)
	const space = cid("entity", s.space)
	const put = (property: string, key: bigint | number, value: string) =>
		putIntent({id: valueStatementId(record, property, key), entity: record, property, value})
	const n = (property: string, v: bigint) => put(property, v, `.{ .number = ${v} }`)
	return [
		`.{ .create_entity = "${record}" }`,
		...keyColumn(recordStatementId(KIND, [idPart(record), idPart(COL.space), idPart(space)]), record, COL.space, `.{ .reference = "${space}" }`),
		n(COL.partial, s.partial),
		n(COL.universal, s.universal),
		n(COL.flat, s.flat),
		n(COL.quorum, s.quorum),
		n(COL.duration, s.duration),
		put(COL.disableFastPath, s.disableFastPath ? 1 : 0, `.{ .boolean = ${s.disableFastPath} }`),
		n(COL.grace, s.grace),
		put(COL.updatedAt, s.updatedAtSeconds, `.{ .instant = "${s.updatedAtSeconds * 1_000_000n}" }`),
		n(COL.updatedAtBlock, s.updatedAtBlock),
	]
}

export type KeyedSpaceVotingSettingsDeps = KeyedRecordDeps

export async function applySpaceVotingSettings(
	deps: KeyedSpaceVotingSettingsDeps,
	s: SpaceVotingSettings,
	meta: {partition: number; offset: string},
): Promise<KeyedOutcome> {
	// Every record lives in the one governance Space; a founding failure is thrown, never dead-lettered.
	return applyKeyedRecord(deps, FAMILY, spaceVotingSettingsIntents(s), {...meta, space: s.space})
}

const headerText = (h: unknown): string | undefined => {
	const v = Array.isArray(h) ? h[0] : h
	if (v === undefined || v === null) return undefined
	return Buffer.isBuffer(v) || v instanceof Uint8Array ? Buffer.from(v).toString("utf8") : String(v)
}

export async function handleSpaceVotingSettingsMessage(
	deps: KeyedSpaceVotingSettingsDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<KeyedOutcome> {
	if (headerText(message.headers?.["event-type"]) !== VOTING_SETTINGS_UPDATED) return "ignored"
	if (!message.value) return "ignored"
	let s: SpaceVotingSettings
	try {
		s = parseVotingSettingsUpdated(message.value)
	} catch (e) {
		return deadLetterDecode(deps, "space.governance", partition, message.offset, e)
	}
	return applySpaceVotingSettings(deps, s, {partition, offset: message.offset})
}

/** Whether the keyed settings writer runs: GEODB_SPACE_VOTING_SETTINGS only takes effect with GEODB_RECORD_KEYS. */
export const spaceVotingSettingsKeyedEnabled = (c: {recordKeys: boolean; spaceVotingSettings: {enabled: boolean}}): boolean =>
	c.spaceVotingSettings.enabled && c.recordKeys

/** The consumer's offsets directory under DATA_DIR; the backfill gates on it (keyedBackfillGate). */
export const spaceVotingSettingsOffsetsDir = (dataDir: string) => `${dataDir}/feeder/space-voting-settings-keyed`

/** A `space_voting_settings` row as the backfill reads it (pgSource.readSpaceVotingSettingsRows). */
export type SpaceVotingSettingsRow = {
	space_id: string
	partial_percentage_support_threshold: string
	universal_percentage_support_threshold: string
	flat_support_threshold: string
	quorum: string
	duration: string
	disable_fast_path_access_for_new_members: boolean
	execution_grace_period: string
	updated_at: string
	updated_at_block: string
}

const I64 = /^-?\d+$/
const big = (k: string, v: string): bigint => {
	if (!I64.test(v) || BigInt.asIntN(64, BigInt(v)) !== BigInt(v)) throw new Error(`space_voting_settings: ${k} ${JSON.stringify(v)}`)
	return BigInt(v)
}

/** A Postgres row as the event that set it. Throws on a value no event could have written. */
export function spaceVotingSettingsFromRow(r: SpaceVotingSettingsRow): SpaceVotingSettings {
	if (typeof r.disable_fast_path_access_for_new_members !== "boolean") throw new Error("space_voting_settings: disable_fast_path_access_for_new_members")
	return {
		space: r.space_id.replaceAll("-", "").toLowerCase(),
		partial: big("partial_percentage_support_threshold", r.partial_percentage_support_threshold),
		universal: big("universal_percentage_support_threshold", r.universal_percentage_support_threshold),
		flat: big("flat_support_threshold", r.flat_support_threshold),
		quorum: big("quorum", r.quorum),
		duration: big("duration", r.duration),
		disableFastPath: r.disable_fast_path_access_for_new_members,
		grace: big("execution_grace_period", r.execution_grace_period),
		updatedAtSeconds: big("updated_at", r.updated_at),
		updatedAtBlock: big("updated_at_block", r.updated_at_block),
	}
}

/**
 * Write a snapshot of `space_voting_settings`. Rerunnable: a held row is written again to the same ids. A row Postgres
 * holds that no event could have written is dead-lettered (counted, the Job fails at the end) rather than invented.
 */
export const backfillSpaceVotingSettings = (
	deps: KeyedSpaceVotingSettingsDeps,
	rows: AsyncIterable<SpaceVotingSettingsRow> | Iterable<SpaceVotingSettingsRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{topic: "space.governance", fromRow: spaceVotingSettingsFromRow, apply: (d, s) => applySpaceVotingSettings(d, s, {partition: -1, offset: "backfill"})},
		progress,
	)
