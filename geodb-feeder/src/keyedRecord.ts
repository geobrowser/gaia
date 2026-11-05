/**
 * What every keyed-record family (votesKeyed.ts, proposalVotesKeyed.ts) shares: the key-column intent pair, applying
 * one record's intents in its family's system Space, and the backfill loop over Postgres rows.
 */
import {type ApplyDeps, applyIntents} from "./apply.js"
import type {KeyedOffsets} from "./membershipKeyed.js"
import type {SystemFamily} from "./recordKinds.js"
import {ensureSystemSpace, type SystemSpaceDeps} from "./systemSpaces.js"

export type KeyedRecordDeps = ApplyDeps & SystemSpaceDeps & {offsets: KeyedOffsets}

export type KeyedOutcome = "applied" | "ignored" | "dead_letter"

/** A key column: written once per record, its statement id fixed by the target, so a replay re-adds the same id. */
export const keyColumn = (id: string, record: string, property: string, value: string): string[] => [
	`.{ .add = .{ .id = "${id}", .entity = "${record}", .property = "${property}", .value = ${value} } }`,
	`.{ .set_active = .{ .statement = "${id}", .active = true } }`,
]

/**
 * Apply one record's intents in its family's system Space. A founding failure, ResourceExhausted included
 * (SystemSpaceCapError), is thrown, never dead-lettered: the consumer retries the message without marking its
 * offset and the backfill Job fails. Skipping would drop the whole family, not one record. A permanently refused
 * intent is dead-lettered on its own (applyIntents bisects) and the rest are written, leaving the record partial.
 */
export async function applyKeyedRecord(
	deps: KeyedRecordDeps,
	family: SystemFamily,
	intents: string[],
	meta: {partition: number; offset: string; space: string},
): Promise<KeyedOutcome> {
	// Founding is inside the lock, as the applies are: two callers (the consumer and a backfill page) must not race it.
	const space = await (deps.lock ?? ((f) => f()))(() => ensureSystemSpace(deps, family))
	const applyDeps: ApplyDeps = {...deps, saveSpace: () => deps.systemSpaces.save()}
	const r = await applyIntents(applyDeps, space, meta, intents, intents.length, 0, () => {})
	return r.deadLettered > 0 ? "dead_letter" : "applied"
}

/** Dead-letter one message or row that no event could have produced. */
export function deadLetterDecode(deps: KeyedRecordDeps, topic: string, partition: number, offset: string, e: unknown): "dead_letter" {
	deps.store.deadLetter({partition, offset, topic, decode: String(e instanceof Error ? e.message : e)})
	deps.metrics.deadLetters.inc({kind: "message"})
	return "dead_letter"
}

export type KeyedBackfillResult = {rows: number; applied: number; deadLetter: number}

/**
 * Write a snapshot of one table. Rerunnable: a held row is written again to the same ids. A row Postgres holds that
 * no event could have written is dead-lettered (counted; the Job fails at the end) rather than invented.
 */
export async function backfillKeyedRows<R, E>(
	deps: KeyedRecordDeps,
	rows: AsyncIterable<R> | Iterable<R>,
	opts: {topic: string; fromRow: (r: R) => E; apply: (deps: KeyedRecordDeps, e: E) => Promise<KeyedOutcome>},
	progress: (done: number) => void = () => {},
): Promise<KeyedBackfillResult> {
	const out: KeyedBackfillResult = {rows: 0, applied: 0, deadLetter: 0}
	for await (const r of rows) {
		out.rows++
		let e: E
		try {
			e = opts.fromRow(r)
		} catch (err) {
			deadLetterDecode(deps, opts.topic, -1, "backfill", err)
			out.deadLetter++
			continue
		}
		const o = await opts.apply(deps, e)
		if (o === "dead_letter") out.deadLetter++
		else out.applied++
		progress(out.rows)
		if (deps.signal?.aborted) throw new Error(`${opts.topic} backfill aborted`)
	}
	return out
}
