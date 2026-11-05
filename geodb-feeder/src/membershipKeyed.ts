/**
 * Space membership as natural-key records (geo-db plan 10.12; plan 10.11 GAP-7), behind GEODB_RECORD_KEYS. It
 * replaces the journal-planned dual write in membership.ts when the flag is on; with the flag off nothing here runs.
 *
 * Model, in the `governance` system Space (systemSpaces.ts), one edge row per (Space, role, member) (K2):
 * - subject: the Space's own entity, `cid("entity", spaceId)`, as in the legacy path;
 * - property: the same `ROLE_PROPERTY` ids, so readers that already filter membership out of a Space card
 *   (api/src/geodb/card.ts MEMBERSHIP_PROPERTIES) need no change;
 * - value: a reference to `cid("entity", memberSpaceId)` (K5: the target need not exist in this Space);
 * - statement id: `recordStatementId(RECORD_KINDS.membership, [subject, property, target])`.
 *
 * A grant is `create_entity(subject)`, `add(sid, edge)`, `set_active(sid, true)`; a revoke is
 * `set_active(sid, false)`. Each intent is idempotent in the engine and a retract of an id the store never saw is
 * admitted (K1), so every event's intents are fixed by the event alone: no journal, no read before write, and a
 * replay from offset zero, a rerun backfill or a redelivered message all converge on the last event per key.
 */
import {type ApplyDeps, applyIntents} from "./apply.js"
import {cid} from "./ids.js"
import {type MembershipOutcome, parseRoleEvent, ROLE_PROPERTY, type Role, type RoleEvent, subjectEntity} from "./membership.js"
import type {CompareTarget} from "./membershipShadow.js"
import {idPart, recordStatementId} from "./recordKey.js"
import {RECORD_KINDS} from "./recordKinds.js"
import {ensureSystemSpace, SystemSpaceCapError, type SystemSpaceDeps} from "./systemSpaces.js"

const FAMILY = RECORD_KINDS.membership.family

/** The keyed statement id of one (Space, role, member) edge. */
export function keyedStatementId(space: string, member: string, role: Role): string {
	return recordStatementId(RECORD_KINDS.membership.id, [
		idPart(subjectEntity(space)),
		idPart(ROLE_PROPERTY[role]),
		idPart(cid("entity", member)),
	])
}

/** The intents for one event. Pure, and depends on the event alone. */
export function keyedRoleIntents(ev: RoleEvent): string[] {
	const sid = keyedStatementId(ev.space, ev.member, ev.role)
	if (ev.kind === "revoke") return [`.{ .set_active = .{ .statement = "${sid}", .active = false } }`]
	const subject = subjectEntity(ev.space)
	return [
		`.{ .create_entity = "${subject}" }`,
		`.{ .add = .{ .id = "${sid}", .entity = "${subject}", .property = "${ROLE_PROPERTY[ev.role]}", .value = .{ .reference = "${cid("entity", ev.member)}" } } }`,
		`.{ .set_active = .{ .statement = "${sid}", .active = true } }`,
	]
}

/** Offsets only: what geo-db holds is derived from the event, so there is nothing else to remember. */
export type KeyedOffsets = {
	readonly hadOffsets: boolean
	lastDone(partition: number): number | null
	markDone(partition: number, offset: number): void
}

export type KeyedMembershipDeps = ApplyDeps & SystemSpaceDeps & {offsets: KeyedOffsets}

export async function applyKeyedRoleEvent(
	deps: KeyedMembershipDeps,
	ev: RoleEvent,
	meta: {partition: number; offset: string},
): Promise<MembershipOutcome> {
	let space: Awaited<ReturnType<typeof ensureSystemSpace>>
	try {
		space = await ensureSystemSpace(deps, FAMILY)
	} catch (e) {
		if (!(e instanceof SystemSpaceCapError)) throw e
		deps.store.deadLetter({...meta, topic: "membership", space: ev.space, err: e.message})
		deps.metrics.deadLetters.inc({kind: "space_cap"})
		return "dead_letter"
	}
	const intents = keyedRoleIntents(ev)
	const applyDeps: ApplyDeps = {...deps, saveSpace: () => deps.systemSpaces.save()}
	const r = await applyIntents(applyDeps, space, {...meta, space: ev.space}, intents, intents.length, 0, () => {})
	return r.deadLettered > 0 ? "dead_letter" : "applied"
}

export async function handleKeyedMembershipMessage(
	deps: KeyedMembershipDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<MembershipOutcome> {
	if (!message.value) return "ignored"
	const header = message.headers?.["event-type"]
	const eventType = header == null ? undefined : Buffer.from(header as Uint8Array | string).toString("utf8")
	let ev: RoleEvent | null
	try {
		ev = parseRoleEvent(eventType, message.value)
	} catch (e) {
		deps.store.deadLetter({partition, offset: message.offset, topic: "membership", decode: String(e instanceof Error ? e.message : e)})
		deps.metrics.deadLetters.inc({kind: "message"})
		return "dead_letter"
	}
	if (!ev) return "ignored"
	return applyKeyedRoleEvent(deps, ev, {partition, offset: message.offset})
}

export type KeyedBackfillResult = {rows: number; applied: number; deadLetter: number}

/**
 * Write a Postgres snapshot of members and editors as grants. Rerunnable: a held row is written again to the same
 * value. It never retracts; a revoke the stream carries after the snapshot is applied by the stream.
 */
export async function backfillMembershipKeyed(
	deps: KeyedMembershipDeps,
	rows: Iterable<{space_id: string; member_space_id: string; role: Role}>,
	progress: (done: number) => void = () => {},
): Promise<KeyedBackfillResult> {
	const out: KeyedBackfillResult = {rows: 0, applied: 0, deadLetter: 0}
	for (const r of rows) {
		out.rows++
		const ev: RoleEvent = {
			kind: "grant",
			space: r.space_id.replaceAll("-", "").toLowerCase(),
			member: r.member_space_id.replaceAll("-", "").toLowerCase(),
			role: r.role,
		}
		const o = await applyKeyedRoleEvent(deps, ev, {partition: -1, offset: "backfill"})
		if (o === "dead_letter") out.deadLetter++
		else out.applied++
		progress(out.rows)
		if (deps.signal?.aborted) throw new Error("membership backfill aborted")
	}
	return out
}

/** The shadow compare's view of the keyed rows: all in the governance Space, under keyed ids. */
export const keyedCompareTarget = (systemSpaces: SystemSpaceDeps["systemSpaces"]): CompareTarget => ({
	geoSpace: () => systemSpaces.records[FAMILY]?.geo,
	statementId: keyedStatementId,
})
