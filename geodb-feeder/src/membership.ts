/**
 * Space membership (members and editors) into geo-db, in parallel to the kg-indexer's `members` / `editors`
 * tables. Postgres stays the source of truth; this is a dual write behind GEODB_MEMBERSHIP_DUAL_WRITE.
 *
 * Source: topic `<env>.space.membership`, HermesRoleGranted / HermesRoleRevoked (event-type header
 * ROLE_GRANTED / ROLE_REVOKED), the same two messages the kg-indexer applies (handlers/membership.rs).
 *
 * Model, in the geo-db Space of the Space that grants the role (chain-derived, sealed by the feeder's sealer):
 * - subject: the Space's own entity, `cid("entity", spaceId)`;
 * - property: one of two feeder-minted property ids, `member` or `editor`;
 * - value: a reference to `cid("entity", memberSpaceId)`, the same mapping every other gaia entity id gets, so a
 *   personal Space id that is also a Person entity id references that entity;
 * - statement id: derived from (space, member, role), so a grant is one statement for ever. Grant is `add` the
 *   first time and `set_active true` after a revoke; revoke is `set_active false` (a retraction, history kept).
 */
import {appendFileSync, existsSync, mkdirSync, readFileSync} from "node:fs"
import {type ApplyDeps, applyIntents} from "./apply.js"
import {conformingId, cid} from "./ids.js"
import {parseFields} from "./hermes.js"
import {ensureSpace, type SpaceDeps} from "./spaces.js"
import {eachLine, writeFileDurable} from "./state.js"

export type Role = "member" | "editor"
export type RoleEvent = {kind: "grant" | "revoke"; space: string; member: string; role: Role}
export type MembershipOutcome = "applied" | "noop" | "ignored" | "dead_letter"

const hex = (v: Uint8Array) => Buffer.from(v).toString("hex")

/** geo-db property ids for the two roles. Minted here (no gaia id exists), deterministic, never change. */
export const ROLE_PROPERTY: Record<Role, string> = {
	member: conformingId("membership-property", "member"),
	editor: conformingId("membership-property", "editor"),
}

export const subjectEntity = (space: string): string => cid("entity", space)

export function statementId(space: string, member: string, role: Role): string {
	return conformingId("membership-statement", `${space}:${member}:${role}`)
}

/**
 * HermesRoleGranted / HermesRoleRevoked (hermes-schema/proto/membership.proto): space_id = 1, member_space_id = 2,
 * role = 3 (EDITOR = 0, MEMBER = 1; proto3 omits a zero, so an absent role is EDITOR). Null for any other
 * event type; throws on a malformed message.
 */
export function parseRoleEvent(eventType: string | undefined, payload: Uint8Array): RoleEvent | null {
	const kind = eventType === "ROLE_GRANTED" ? "grant" : eventType === "ROLE_REVOKED" ? "revoke" : null
	if (!kind) return null
	let space: string | undefined
	let member: string | undefined
	let role = 0n
	for (const [f, v] of parseFields(payload)) {
		if (f === 1 && v instanceof Uint8Array) space = hex(v)
		else if (f === 2 && v instanceof Uint8Array) member = hex(v)
		else if (f === 3 && typeof v === "bigint") role = v
	}
	if (space?.length !== 32 || member?.length !== 32) throw new Error("role event without a 16-byte space and member")
	if (role !== 0n && role !== 1n) throw new Error(`unknown membership role ${role}`)
	return {kind, space, member, role: role === 1n ? "member" : "editor"}
}

type Offsets = {done: Record<string, number>}

/**
 * Durable membership state, apart from the edits consumer's: offsets, and an append-only journal of what geo-db
 * holds (`e <space>|<entity>` a subject entity created, `s <statement> <0|1>` a statement and whether it is active;
 * the last line for a statement wins). A message's journal lines are written only after its apply succeeded.
 */
export class MembershipState {
	private readonly offsetsPath: string
	private readonly journalPath: string
	private offsets: Offsets
	private readonly entities = new Set<string>()
	private readonly statements = new Map<string, boolean>()
	readonly hadOffsets: boolean

	constructor(readonly dir: string) {
		mkdirSync(dir, {recursive: true})
		this.offsetsPath = `${dir}/offsets.json`
		this.journalPath = `${dir}/journal.log`
		this.hadOffsets = existsSync(this.offsetsPath)
		this.offsets = this.hadOffsets ? {done: {}, ...JSON.parse(readFileSync(this.offsetsPath, "utf8"))} : {done: {}}
		eachLine(this.journalPath, (l) => {
			if (l.startsWith("e ")) this.entities.add(l.slice(2))
			else if (l.startsWith("s ")) {
				const [, id, a] = l.split(" ")
				if (id) this.statements.set(id, a === "1")
			}
		})
	}

	hasEntity = (space: string, entity: string) => this.entities.has(`${space}|${entity}`)
	/** undefined: geo-db holds no such statement; else whether it is active. */
	statement = (id: string): boolean | undefined => this.statements.get(id)
	get statementCount(): number {
		return this.statements.size
	}
	/** The gaia Spaces the journal holds a subject entity for. */
	spaces(): string[] {
		return [...new Set([...this.entities].map((e) => e.split("|")[0] ?? ""))].filter(Boolean)
	}

	commit(entities: [string, string][], statements: [string, boolean][]): void {
		const text =
			entities.map(([sp, e]) => `e ${sp}|${e}\n`).join("") + statements.map(([id, a]) => `s ${id} ${a ? 1 : 0}\n`).join("")
		if (text) appendFileSync(this.journalPath, text)
		for (const [sp, e] of entities) this.entities.add(`${sp}|${e}`)
		for (const [id, a] of statements) this.statements.set(id, a)
	}

	lastDone(partition: number): number | null {
		return this.offsets.done[String(partition)] ?? null
	}

	markDone(partition: number, offset: number): void {
		this.offsets.done[String(partition)] = offset
		writeFileDurable(this.offsetsPath, JSON.stringify(this.offsets))
	}
}

export type Planned = {intents: string[]; entities: [string, string][]; statements: [string, boolean][]}

/** The intents that bring geo-db to the state `ev` says, given what the journal holds. Pure. */
export function planRoleEvent(ev: RoleEvent, state: Pick<MembershipState, "hasEntity" | "statement">): Planned {
	const out: Planned = {intents: [], entities: [], statements: []}
	const subject = subjectEntity(ev.space)
	const target = cid("entity", ev.member)
	const sid = statementId(ev.space, ev.member, ev.role)
	const held = state.statement(sid)
	if (ev.kind === "grant") {
		if (held === true) return out
		if (held === false) {
			out.intents.push(`.{ .set_active = .{ .statement = "${sid}", .active = true } }`)
			out.statements.push([sid, true])
			return out
		}
		if (!state.hasEntity(ev.space, subject)) {
			out.intents.push(`.{ .create_entity = "${subject}" }`)
			out.entities.push([ev.space, subject])
		}
		out.intents.push(
			`.{ .add = .{ .id = "${sid}", .entity = "${subject}", .property = "${ROLE_PROPERTY[ev.role]}", .value = .{ .reference = "${target}" } } }`,
		)
		out.statements.push([sid, true])
		return out
	}
	// A revoke of a role geo-db never held (the grant predates the topic) or already retracted: nothing to retract.
	if (held !== true) return out
	out.intents.push(`.{ .set_active = .{ .statement = "${sid}", .active = false } }`)
	out.statements.push([sid, false])
	return out
}

export type MembershipDeps = ApplyDeps & SpaceDeps & {state: MembershipState}

/** Apply one role event to geo-db and journal it. Safe to repeat: the journal says what is already held. */
export async function applyRoleEvent(
	deps: MembershipDeps,
	ev: RoleEvent,
	meta: {partition: number; offset: string},
): Promise<MembershipOutcome> {
	const plan = planRoleEvent(ev, deps.state)
	if (plan.intents.length === 0) return "noop"
	const space = await ensureSpace(deps, ev.space)
	const r = await applyIntents(deps, space, {...meta, space: ev.space}, plan.intents, plan.intents.length, 0, () => {})
	// A refused intent was dead-lettered (errors.jsonl) by applyIntents. The journal is then left as it was, so
	// the journal never claims a statement geo-db refused; a later event for the same role plans it again.
	if (r.deadLettered > 0) return "dead_letter"
	deps.state.commit(plan.entities, plan.statements)
	return "applied"
}

export async function handleMembershipMessage(
	deps: MembershipDeps,
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
	return applyRoleEvent(deps, ev, {partition, offset: message.offset})
}

export type BackfillResult = {rows: number; applied: number; noop: number; deadLetter: number}

/**
 * Bring geo-db up to a Postgres snapshot of members and editors: every row is a grant, applied through the same
 * path as a live event, so rows geo-db already holds are no-ops and a rerun resumes where it stopped. It adds
 * and re-activates only; it never retracts, so run it with the live consumer stopped or accept that a revoke the
 * stream carries after the snapshot is applied by the stream.
 */
export async function backfillMembership(
	deps: MembershipDeps,
	rows: Iterable<{space_id: string; member_space_id: string; role: Role}>,
	progress: (done: number) => void = () => {},
): Promise<BackfillResult> {
	const out: BackfillResult = {rows: 0, applied: 0, noop: 0, deadLetter: 0}
	for (const r of rows) {
		out.rows++
		const ev: RoleEvent = {
			kind: "grant",
			space: r.space_id.replaceAll("-", "").toLowerCase(),
			member: r.member_space_id.replaceAll("-", "").toLowerCase(),
			role: r.role,
		}
		const o = await applyRoleEvent(deps, ev, {partition: -1, offset: "backfill"})
		if (o === "noop") out.noop++
		else if (o === "dead_letter") out.deadLetter++
		else out.applied++
		progress(out.rows)
		if (deps.signal?.aborted) throw new Error("membership backfill aborted")
	}
	return out
}
