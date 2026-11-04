import {mkdtempSync, readFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {applyIntents, createLock} from "../apply.js"
import {cid} from "../ids.js"
import {
	ROLE_PROPERTY,
	MembershipState,
	applyRoleEvent,
	backfillMembership,
	handleMembershipMessage,
	parseRoleEvent,
	planRoleEvent,
	statementId,
	subjectEntity,
	type MembershipDeps,
	type RoleEvent,
} from "../membership.js"
import {DIFF_CAP, compareMembership} from "../membershipShadow.js"
import {createMetrics} from "../metrics.js"
import {Store} from "../state.js"

const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const ALICE = "b29c345ab9866679b001d7d2138d88a2"
const BOB = "c39c345ab9866679b001d7d2138d88a3"
const GEO = "daf0bc83ca744e1a9ee0377967846846"
const SEALER = "5".repeat(64)
const tmp = () => mkdtempSync(join(tmpdir(), "membership-"))

const bytes = (h: string) => Buffer.from(h, "hex")
/** HermesRoleGranted/Revoked: space_id = 1, member_space_id = 2, role = 3 (omitted when 0, as proto3 does). */
function roleMessage(space: string, member: string, role: 0 | 1): Uint8Array {
	const field = (n: number, b: Buffer) => Buffer.concat([Buffer.from([(n << 3) | 2, b.length]), b])
	const parts = [field(1, bytes(space)), field(2, bytes(member))]
	if (role !== 0) parts.push(Buffer.from([(3 << 3) | 0, role]))
	return Buffer.concat(parts)
}

const grant = (member: string, role: "member" | "editor" = "member", space = SPACE): RoleEvent => ({kind: "grant", space, member, role})
const revoke = (member: string, role: "member" | "editor" = "member", space = SPACE): RoleEvent => ({kind: "revoke", space, member, role})

function fixture() {
	const store = new Store(tmp())
	const state = new MembershipState(`${store.dir}/membership`)
	const applied: string[] = []
	const founds: string[] = []
	const deps: MembershipDeps = {
		db: {
			apply: async (zon: string) => {
				applied.push(zon)
				return {rc: 0, out: "", err: ""}
			},
			waitReady: async () => [],
			found: async (_s: string, nonce: string) => {
				founds.push(nonce)
				return GEO
			},
		},
		sealer: async () => SEALER,
		store,
		metrics: createMetrics(),
		beat: async () => {},
		sleep: async () => {},
		publish: () => {},
		state,
	}
	return {store, state, applied, founds, deps}
}

describe("parseRoleEvent", () => {
	it("reads grant and revoke, and treats an absent role as EDITOR (proto3 zero)", () => {
		expect(parseRoleEvent("ROLE_GRANTED", roleMessage(SPACE, ALICE, 1))).toEqual(grant(ALICE, "member"))
		expect(parseRoleEvent("ROLE_REVOKED", roleMessage(SPACE, ALICE, 0))).toEqual(revoke(ALICE, "editor"))
	})
	it("ignores other event types and rejects a malformed message", () => {
		expect(parseRoleEvent("SPACE_LEFT", roleMessage(SPACE, ALICE, 1))).toBeNull()
		expect(parseRoleEvent(undefined, roleMessage(SPACE, ALICE, 1))).toBeNull()
		expect(() => parseRoleEvent("ROLE_GRANTED", Buffer.from([0x0a, 0x01, 0x01]))).toThrow()
		expect(() => parseRoleEvent("ROLE_GRANTED", Buffer.concat([roleMessage(SPACE, ALICE, 1).slice(0, -2), Buffer.from([0x18, 7])]))).toThrow(/role/)
	})
})

describe("ids", () => {
	it("are deterministic, v4-shaped, and distinct per role and per member", () => {
		const a = statementId(SPACE, ALICE, "member")
		expect(a).toBe(statementId(SPACE, ALICE, "member"))
		expect(a).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
		expect(new Set([a, statementId(SPACE, ALICE, "editor"), statementId(SPACE, BOB, "member")]).size).toBe(3)
		expect(ROLE_PROPERTY.member).not.toBe(ROLE_PROPERTY.editor)
		expect(ROLE_PROPERTY.member).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
	})
})

describe("planRoleEvent", () => {
	it("first grant creates the Space entity and adds one reference statement", () => {
		const f = fixture()
		const p = planRoleEvent(grant(ALICE, "editor"), f.state)
		expect(p.intents).toEqual([
			`.{ .create_entity = "${subjectEntity(SPACE)}" }`,
			`.{ .add = .{ .id = "${statementId(SPACE, ALICE, "editor")}", .entity = "${subjectEntity(SPACE)}", .property = "${ROLE_PROPERTY.editor}", .value = .{ .reference = "${cid("entity", ALICE)}" } } }`,
		])
	})
	it("a revoke of a role geo-db never held is a no-op", () => {
		expect(planRoleEvent(revoke(ALICE), fixture().state).intents).toEqual([])
	})
})

describe("applyRoleEvent", () => {
	it("grant, revoke, re-grant is add, set_active false, set_active true; repeats are no-ops", async () => {
		const f = fixture()
		const sid = statementId(SPACE, ALICE, "member")
		const meta = {partition: 0, offset: "1"}
		expect(await applyRoleEvent(f.deps, grant(ALICE), meta)).toBe("applied")
		expect(await applyRoleEvent(f.deps, grant(ALICE), meta)).toBe("noop")
		expect(await applyRoleEvent(f.deps, revoke(ALICE), meta)).toBe("applied")
		expect(await applyRoleEvent(f.deps, revoke(ALICE), meta)).toBe("noop")
		expect(await applyRoleEvent(f.deps, grant(ALICE), meta)).toBe("applied")
		expect(f.applied).toHaveLength(3)
		expect(f.applied[0]).toContain(".add")
		expect(f.applied[0]).toContain('.key = "') // the Space's founding edit carries its key
		expect(f.applied[1]).toContain(`.set_active = .{ .statement = "${sid}", .active = false }`)
		expect(f.applied[2]).toContain(`.set_active = .{ .statement = "${sid}", .active = true }`)
		expect(f.founds).toHaveLength(1)
	})

	it("replays from the journal after a restart without re-adding", async () => {
		const f = fixture()
		await applyRoleEvent(f.deps, grant(ALICE), {partition: 0, offset: "1"})
		const reopened = new MembershipState(f.state.dir)
		expect(reopened.statement(statementId(SPACE, ALICE, "member"))).toBe(true)
		expect(planRoleEvent(grant(ALICE), reopened).intents).toEqual([])
		expect(reopened.spaces()).toEqual([SPACE])
	})

	it("a refused intent is dead-lettered and not journaled", async () => {
		const f = fixture()
		f.deps.db.apply = async () => ({rc: 1, out: "", err: "InvalidIntent"})
		expect(await applyRoleEvent(f.deps, grant(ALICE), {partition: 0, offset: "9"})).toBe("dead_letter")
		expect(f.state.statement(statementId(SPACE, ALICE, "member"))).toBeUndefined()
		expect(readFileSync(`${f.store.dir}/errors.jsonl`, "utf8")).toContain('"offset":"9"')
	})
})

describe("space cap", () => {
	const exhausted = async () => {
		throw new Error("found n: rc=1 Refused: ResourceExhausted")
	}
	it("dead-letters an event whose Space geo-db cannot found, and asks geo-db once", async () => {
		const f = fixture()
		let asks = 0
		f.deps.db.found = async () => {
			asks++
			return exhausted()
		}
		expect(await applyRoleEvent(f.deps, grant(ALICE), {partition: 0, offset: "3"})).toBe("dead_letter")
		expect(await applyRoleEvent(f.deps, grant(BOB), {partition: 0, offset: "4"})).toBe("dead_letter")
		expect(asks).toBe(1)
		expect(f.applied).toEqual([])
		expect(f.state.statement(statementId(SPACE, ALICE, "member"))).toBeUndefined()
		const lines = readFileSync(`${f.store.dir}/errors.jsonl`, "utf8").trim().split("\n").map((l) => JSON.parse(l))
		expect(lines).toHaveLength(2)
		expect(lines[0]).toMatchObject({kind: "space_cap", space: SPACE, offset: "3", topic: "membership"})
	})

	it("a transient founding failure still throws, and is retried on the next call", async () => {
		const f = fixture()
		f.deps.db.found = async () => {
			throw new Error("found n: rc=1 ConnectionRefused")
		}
		await expect(applyRoleEvent(f.deps, grant(ALICE), {partition: 0, offset: "3"})).rejects.toThrow(/ConnectionRefused/)
		f.deps.db.found = async () => GEO
		expect(await applyRoleEvent(f.deps, grant(ALICE), {partition: 0, offset: "3"})).toBe("applied")
	})
})

describe("handleMembershipMessage", () => {
	it("routes on the event-type header", async () => {
		const f = fixture()
		const h = (t: string) => ({"event-type": Buffer.from(t)})
		const m = (value: Uint8Array | null, headers?: Record<string, unknown>) => ({offset: "5", value, headers})
		expect(await handleMembershipMessage(f.deps, 0, m(roleMessage(SPACE, BOB, 1), h("ROLE_GRANTED")))).toBe("applied")
		expect(await handleMembershipMessage(f.deps, 0, m(roleMessage(SPACE, BOB, 1), h("SPACE_LEFT")))).toBe("ignored")
		expect(await handleMembershipMessage(f.deps, 0, m(null))).toBe("ignored")
		expect(await handleMembershipMessage(f.deps, 0, m(Buffer.from([1]), h("ROLE_GRANTED")))).toBe("dead_letter")
		expect(await handleMembershipMessage(f.deps, 0, m(roleMessage(SPACE, BOB, 1), h("ROLE_REVOKED")))).toBe("applied")
	})
})

describe("backfillMembership", () => {
	it("applies every row once, and a rerun is all no-ops", async () => {
		const f = fixture()
		const dashed = (h: string) => `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`
		const rows = [
			{space_id: dashed(SPACE), member_space_id: dashed(ALICE), role: "member" as const},
			{space_id: dashed(SPACE), member_space_id: dashed(ALICE), role: "editor" as const},
			{space_id: dashed(SPACE), member_space_id: dashed(BOB), role: "member" as const},
		]
		expect(await backfillMembership(f.deps, rows)).toEqual({rows: 3, applied: 3, noop: 0, deadLetter: 0})
		expect(await backfillMembership(f.deps, rows)).toEqual({rows: 3, applied: 0, noop: 3, deadLetter: 0})
		expect(f.applied).toHaveLength(3)
	})

	it("a rerun in a new process (journal reloaded from disk) is all no-ops", async () => {
		const f = fixture()
		const rows = [
			{space_id: SPACE, member_space_id: ALICE, role: "member" as const},
			{space_id: SPACE, member_space_id: BOB, role: "editor" as const},
		]
		await backfillMembership(f.deps, rows)
		const again = {...f.deps, state: new MembershipState(f.state.dir)}
		expect(await backfillMembership(again, rows)).toEqual({rows: 2, applied: 0, noop: 2, deadLetter: 0})
		expect(f.applied).toHaveLength(2)
	})

	it("re-activates a role retracted since, and counts a refused row as a dead letter, not applied", async () => {
		const f = fixture()
		const rows = [{space_id: SPACE, member_space_id: ALICE, role: "member" as const}]
		await backfillMembership(f.deps, rows)
		await applyRoleEvent(f.deps, revoke(ALICE), {partition: 0, offset: "1"})
		expect(await backfillMembership(f.deps, rows)).toMatchObject({applied: 1, noop: 0})
		expect(f.applied.at(-1)).toContain(".active = true")

		const g = fixture()
		g.deps.db.apply = async () => ({rc: 1, out: "", err: "refused"})
		const r = await backfillMembership(g.deps, rows)
		expect(r).toMatchObject({rows: 1, applied: 0, deadLetter: 1})
		expect(g.state.statementCount).toBe(0)
	})
})

describe("compareMembership", () => {
	const sid = (member: string, role: "member" | "editor" = "member", space = SPACE) => statementId(space, member, role)
	const geoRows = (ids: string[], prop: string) => ids.map((statement) => ({prop, value: "v", statement, entity: subjectEntity(SPACE)}))

	it("counts match, mismatch, unmapped and error per role and sets the metrics", async () => {
		const metrics = createMetrics()
		const OTHER = "e49c345ab9866679b001d7d2138d88a4"
		const reader = {
			reset: () => {},
			entityRows: async (_geo: string, entity: string) => {
				if (entity === subjectEntity(OTHER)) throw new Error("boom")
				return [...geoRows([sid(ALICE), sid(BOB)], ROLE_PROPERTY.member), ...geoRows([sid(ALICE, "editor")], ROLE_PROPERTY.editor)]
			},
		}
		const GONE = "f59c345ab9866679b001d7d2138d88a5"
		const s = await compareMembership({
			pgMembers: async () =>
				new Map([
					[SPACE, {member: [ALICE, BOB], editor: [ALICE, BOB]}], // editor BOB missing in geo
					[OTHER, {member: [ALICE], editor: []}], // read fails
					[GONE, {member: [ALICE], editor: []}], // no geo Space
				]),
			reader,
			spaces: () => ({
				[SPACE]: {geo: GEO, nonce: "n", keyed: true},
				[OTHER]: {geo: GEO, nonce: "n", keyed: true},
			}),
			journalSpaces: () => [SPACE],
			metrics,
			sleep: async () => {},
		})
		expect(s.mismatched).toEqual([{space: SPACE, role: "editor", pg: 2, geo: 1, missing: [BOB], extra: []}])
		expect(s).toMatchObject({compared: 2, unmapped: 1, errors: 1})
		const text = metrics.render()
		expect(text).toContain('geodb_feeder_membership_shadow_compares_total{result="match",role="member"} 1')
		expect(text).toContain('geodb_feeder_membership_shadow_compares_total{result="mismatch",role="editor"} 1')
		expect(text).toContain('geodb_feeder_membership_shadow_spaces{result="mismatch",role="editor"} 1')
		expect(text).toContain('geodb_feeder_membership_shadow_spaces{result="unmapped",role="member"} 1')
		expect(text).toContain('geodb_feeder_membership_shadow_diff_ids{role="editor",side="missing_in_geo"} 1')
		expect(text).toContain('geodb_feeder_membership_shadow_diff_ids{role="member",side="extra_in_geo"} 0')
	})

	it("equal counts with different members is a mismatch, naming both sides' ids", async () => {
		const metrics = createMetrics()
		const s = await compareMembership({
			pgMembers: async () => new Map([[SPACE, {member: [ALICE], editor: []}]]),
			reader: {reset: () => {}, entityRows: async () => geoRows([sid(BOB)], ROLE_PROPERTY.member)},
			spaces: () => ({[SPACE]: {geo: GEO, nonce: "n", keyed: true}}),
			journalSpaces: () => [],
			metrics,
		})
		expect(s.mismatched).toEqual([{space: SPACE, role: "member", pg: 1, geo: 1, missing: [ALICE], extra: [sid(BOB)]}])
		expect(metrics.render()).toContain('geodb_feeder_membership_shadow_diff_ids{role="member",side="extra_in_geo"} 1')
	})

	it("caps the ids named per mismatch but counts them all in the metric", async () => {
		const metrics = createMetrics()
		const many = Array.from({length: 12}, (_, i) => `${String(i).padStart(2, "0")}9c345ab9866679b001d7d2138d88a1`)
		const s = await compareMembership({
			pgMembers: async () => new Map([[SPACE, {member: many, editor: []}]]),
			reader: {reset: () => {}, entityRows: async () => []},
			spaces: () => ({[SPACE]: {geo: GEO, nonce: "n", keyed: true}}),
			journalSpaces: () => [],
			metrics,
		})
		expect(s.mismatched[0]?.missing).toHaveLength(DIFF_CAP)
		expect(metrics.render()).toContain('geodb_feeder_membership_shadow_diff_ids{role="member",side="missing_in_geo"} 12')
	})

	it("compares a Space Postgres no longer has rows for against the empty set", async () => {
		const s = await compareMembership({
			pgMembers: async () => new Map(),
			reader: {reset: () => {}, entityRows: async () => geoRows([sid(ALICE)], ROLE_PROPERTY.member)},
			spaces: () => ({[SPACE]: {geo: GEO, nonce: "n", keyed: true}}),
			journalSpaces: () => [SPACE],
			metrics: createMetrics(),
		})
		expect(s.mismatched).toEqual([{space: SPACE, role: "member", pg: 0, geo: 1, missing: [], extra: [sid(ALICE)]}])
	})
})

describe("createLock", () => {
	it("serializes two consumers' first applies so only the first carries the founding key", async () => {
		const f = fixture()
		const lock = createLock()
		const space = {geo: GEO, nonce: "n", keyed: false}
		let inflight = 0
		let maxInflight = 0
		const db = {
			waitReady: async () => [],
			apply: async (zon: string) => {
				inflight++
				maxInflight = Math.max(maxInflight, inflight)
				await new Promise((r) => setTimeout(r, 5))
				f.applied.push(zon)
				inflight--
				return {rc: 0, out: "", err: ""}
			},
		}
		const deps = {db, store: f.store, metrics: createMetrics(), beat: async () => {}, lock}
		const meta = {space: SPACE, partition: 0, offset: "1"}
		await Promise.all([
			applyIntents(deps, space, meta, ['.{ .create_entity = "a" }'], 10, 0, () => {}),
			applyIntents(deps, space, meta, ['.{ .create_entity = "b" }'], 10, 0, () => {}),
		])
		expect(maxInflight).toBe(1)
		expect(f.applied.filter((z) => z.includes(".key"))).toHaveLength(1)
	})
})
