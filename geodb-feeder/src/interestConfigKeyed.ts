/**
 * `personalization.interest_config` and `personalization.interest_signal_weights` as natural-key records (geo-db plan
 * 10.12 K1-K5; plan 10.11 GAP-7, the `config` family; inventory 11 §3.4), behind GEODB_INTEREST_CONFIG, which takes
 * effect only with GEODB_RECORD_KEYS (interestConfigKeyedEnabled). With either off nothing here runs.
 *
 * This is a mirror, not a move. Postgres stays the source: every reader is plpgsql (api/drizzle/0100, 0105, 0117; the
 * interest-signal refit, the sweep and user_topic_weights read both tables), and no application code reads either, so
 * there is no api shadow. Nothing writes either table but migrations (0100 seeds, 0105/0106/0107 retune weights) and
 * operator SQL, so there is no event stream to consume: the only writer is the backfill Job
 * (`interest-config-backfill-main.ts`), rerun it after an operator edits a row. It is rerunnable and writes the same ids
 * again.
 *
 * Two kinds, both in the `config` system Space, the shape of forYouConfigKeyed.ts:
 * - interest_config: one record, `recordEntityId(interestConfig, [int(1)])`; key column `id` = 1; mutable columns
 *   (INTEREST_CONFIG_COLUMNS) `put` as a decimal, or a number for the three integers. `sweep_lookback` is an interval; it
 *   is mirrored as whole-or-fractional seconds (`extract(epoch ...)`), a decimal.
 * - interest_signal_weights: one record per `kind`, `recordEntityId(interestSignalWeight, [text(kind)])`; key column
 *   `kind` (text); `weight` (decimal), `decays` (boolean), `note` (text, written only when not null).
 * Each mutable `put` is under `recordStatementId(kind, [record, property, value])`, so a changed value closes the old one
 * and A -> B -> A ends on A. A note changed to null is not cleared (a null has no statement to put); the gate reports it.
 * A kind deleted from Postgres is not removed either: the gate reports it as geo-db-only.
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

const CFG_KIND = RECORD_KINDS.interestConfig.id
const W_KIND = RECORD_KINDS.interestSignalWeight.id
const FAMILY = RECORD_KINDS.interestConfig.family
const CFG = RECORD_COLUMNS.interestConfig
const W = RECORD_COLUMNS.interestSignalWeight

export type InterestConfigKey = Exclude<keyof typeof CFG, "id">

/** Every mirrored `interest_config` column: the row field (Postgres name, `sweep_lookback` as seconds), the RECORD_COLUMNS key, and whether it is an integer. */
export const INTEREST_CONFIG_COLUMNS: readonly {pg: string; key: InterestConfigKey; int: boolean}[] = [
	{pg: "half_life_days", key: "halfLifeDays", int: false},
	{pg: "spread_fraction", key: "spreadFraction", int: false},
	{pg: "spread_max_neighbours", key: "spreadMaxNeighbours", int: true},
	{pg: "cooccurrence_min_support", key: "cooccurrenceMinSupport", int: true},
	{pg: "cooccurrence_max_topics_per_entity", key: "cooccurrenceMaxTopicsPerEntity", int: true},
	{pg: "sweep_lookback_seconds", key: "sweepLookbackSeconds", int: false},
]

export type InterestConfig = Record<InterestConfigKey, number | bigint>
export type InterestSignalWeight = {kind: string; weight: number; decays: boolean; note: string | null}

/** The interest_config record's entity id. */
export const interestConfigRecordId = (): string => recordEntityId(CFG_KIND, [intPart(1)])
/** One signal weight's record entity id. */
export const interestSignalWeightRecordId = (kind: string): string => recordEntityId(W_KIND, [textPart(kind)])

const decimalText = (what: string, v: number): string => {
	const d = floatToDecimal(v)
	if (!d) throw new Error(`${what}: ${v}`)
	return `${d[0]},${d[1]}`
}

/** The intents for the interest_config record. Pure, and depends on the values alone. */
export function interestConfigIntents(c: InterestConfig): string[] {
	const record = interestConfigRecordId()
	const column = ({pg, key, int}: (typeof INTEREST_CONFIG_COLUMNS)[number]): string => {
		const v = c[key]
		const property = CFG[key]
		if (int) {
			if (typeof v !== "bigint") throw new Error(`interest_config: ${pg} ${v}`)
			return putIntent({
				id: recordStatementId(CFG_KIND, [idPart(record), idPart(property), intPart(BigInt.asUintN(64, v))]),
				entity: record,
				property,
				value: `.{ .number = ${v} }`,
			})
		}
		const text = typeof v === "number" ? decimalText(`interest_config: ${pg}`, v) : ""
		if (!text) throw new Error(`interest_config: ${pg} ${v}`)
		return putIntent({
			id: recordStatementId(CFG_KIND, [idPart(record), idPart(property), textPart(text)]),
			entity: record,
			property,
			value: `.{ .decimal = "${text}" }`,
		})
	}
	return [
		`.{ .create_entity = "${record}" }`,
		...keyColumn(recordStatementId(CFG_KIND, [idPart(record), idPart(CFG.id), intPart(1)]), record, CFG.id, ".{ .number = 1 }"),
		...INTEREST_CONFIG_COLUMNS.map(column),
	]
}

/** The intents for one signal weight's record. Pure. */
export function interestSignalWeightIntents(w: InterestSignalWeight): string[] {
	const record = interestSignalWeightRecordId(w.kind)
	const put = (property: string, part: ReturnType<typeof textPart>, value: string) =>
		putIntent({id: recordStatementId(W_KIND, [idPart(record), idPart(property), part]), entity: record, property, value})
	const text = decimalText(`interest_signal_weights ${w.kind}: weight`, w.weight)
	return [
		`.{ .create_entity = "${record}" }`,
		...keyColumn(
			recordStatementId(W_KIND, [idPart(record), idPart(W.kind), textPart(w.kind)]),
			record,
			W.kind,
			`.{ .text = ${zigString(w.kind)} }`,
		),
		put(W.weight, textPart(text), `.{ .decimal = "${text}" }`),
		put(W.decays, intPart(w.decays ? 1 : 0), `.{ .boolean = ${w.decays} }`),
		...(w.note === null ? [] : [put(W.note, textPart(w.note), `.{ .text = ${zigString(w.note)} }`)]),
	]
}

export type KeyedInterestConfigDeps = KeyedRecordDeps

export const applyInterestConfig = (
	deps: KeyedInterestConfigDeps,
	c: InterestConfig,
	meta: {partition: number; offset: string},
): Promise<KeyedOutcome> => applyKeyedRecord(deps, FAMILY, interestConfigIntents(c), {...meta, space: "interest_config"})

export const applyInterestSignalWeight = (
	deps: KeyedInterestConfigDeps,
	w: InterestSignalWeight,
	meta: {partition: number; offset: string},
): Promise<KeyedOutcome> =>
	applyKeyedRecord(deps, FAMILY, interestSignalWeightIntents(w), {...meta, space: "interest_signal_weights"})

/** Whether the keyed writer runs: GEODB_INTEREST_CONFIG only takes effect with GEODB_RECORD_KEYS. */
export const interestConfigKeyedEnabled = (c: {recordKeys: boolean; interestConfig: {enabled: boolean}}): boolean =>
	c.interestConfig.enabled && c.recordKeys

/** The backfill gate's offsets directory under DATA_DIR (there is no consumer, so it stays empty). */
export const interestConfigOffsetsDir = (dataDir: string) => `${dataDir}/feeder/interest-config-keyed`

/** The config row as the backfill reads it (pgSource.readInterestConfigRows): every column as text. */
export type InterestConfigRow = Record<string, string>
/** A weight row as the backfill reads it (pgSource.readInterestSignalWeightRows): `weight` as text. */
export type InterestSignalWeightRow = {kind: string; weight: string; decays: boolean; note: string | null}

/** A Postgres row as a config. Throws on a value the table's checks would have refused. */
export function interestConfigFromRow(r: InterestConfigRow): InterestConfig {
	const out: Partial<InterestConfig> = {}
	for (const {pg, key, int} of INTEREST_CONFIG_COLUMNS) {
		const v = r[pg]
		if (typeof v !== "string") throw new Error(`interest_config: ${pg} missing`)
		if (int) {
			if (!/^-?\d+$/.test(v)) throw new Error(`interest_config: ${pg} ${JSON.stringify(v)}`)
			out[key] = BigInt(v)
		} else {
			const n = Number(v)
			if (v.trim() === "" || !Number.isFinite(n)) throw new Error(`interest_config: ${pg} ${JSON.stringify(v)}`)
			out[key] = n
		}
	}
	return out as InterestConfig
}

/** A Postgres row as a signal weight. Throws on what the table could not hold. */
export function interestSignalWeightFromRow(r: InterestSignalWeightRow): InterestSignalWeight {
	const n = Number(r.weight)
	if (typeof r.kind !== "string" || r.kind === "") throw new Error(`interest_signal_weights: kind ${JSON.stringify(r.kind)}`)
	if (typeof r.weight !== "string" || r.weight.trim() === "" || !Number.isFinite(n))
		throw new Error(`interest_signal_weights: weight ${JSON.stringify(r.weight)}`)
	if (typeof r.decays !== "boolean") throw new Error(`interest_signal_weights: decays ${JSON.stringify(r.decays)}`)
	if (r.note !== null && typeof r.note !== "string") throw new Error(`interest_signal_weights: note ${JSON.stringify(r.note)}`)
	return {kind: r.kind, weight: n, decays: r.decays, note: r.note}
}

/** Write the config row. Rerunnable. A row the table could not hold is dead-lettered, not invented. */
export const backfillInterestConfig = (
	deps: KeyedInterestConfigDeps,
	rows: AsyncIterable<InterestConfigRow> | Iterable<InterestConfigRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{
			topic: "interest_config",
			fromRow: interestConfigFromRow,
			apply: (d, c) => applyInterestConfig(d, c, {partition: -1, offset: "backfill"}),
		},
		progress,
	)

/** Write every signal-weight row. Rerunnable. */
export const backfillInterestSignalWeights = (
	deps: KeyedInterestConfigDeps,
	rows: AsyncIterable<InterestSignalWeightRow> | Iterable<InterestSignalWeightRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{
			topic: "interest_signal_weights",
			fromRow: interestSignalWeightFromRow,
			apply: (d, w) => applyInterestSignalWeight(d, w, {partition: -1, offset: "backfill"}),
		},
		progress,
	)

/** Both tables, config first; the totals are summed. */
export async function backfillInterest(
	deps: KeyedInterestConfigDeps,
	rows: {config: AsyncIterable<InterestConfigRow>; weights: AsyncIterable<InterestSignalWeightRow>},
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> {
	const a = await backfillInterestConfig(deps, rows.config, progress)
	const b = await backfillInterestSignalWeights(deps, rows.weights, progress)
	return {rows: a.rows + b.rows, applied: a.applied + b.applied, deadLetter: a.deadLetter + b.deadLetter}
}
