import {mkdtempSync, readFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {applyIntents, assertPutAllowed, isPutIntent} from "../apply.js"
import {loadConfig} from "../config.js"
import {cid} from "../ids.js"
import {ROLE_PROPERTY, type RoleEvent, statementId, subjectEntity} from "../membership.js"
import {
	backfillMembershipKeyed,
	handleKeyedMembershipMessage,
	keyedCompareTarget,
	type KeyedMembershipDeps,
	keyedRoleIntents,
	keyedStatementId,
} from "../membershipKeyed.js"
import {compareMembership} from "../membershipShadow.js"
import {createMetrics} from "../metrics.js"
import {ProposalOffsets} from "../proposals.js"
import {idPart, recordStatementId} from "../recordKey.js"
import {RECORD_KINDS} from "../recordKinds.js"
import {Store} from "../state.js"
import {putIntent} from "../translate.js"
import {ensureSystemSpace, SYSTEM_SPACE_MAP_KEY, SystemSpaceCapError, SystemSpaces, systemSpaceNonce} from "../systemSpaces.js"
import {SpaceMapPublisher} from "../publish.js"

const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const SPACE2 = "e49c345ab9866679b001d7d2138d88a4"
const ALICE = "b29c345ab9866679b001d7d2138d88a2"
const BOB = "c39c345ab9866679b001d7d2138d88a3"
const GOV = "f00dbc83ca744e1a9ee0377967846846"
const SEALER = "5".repeat(64)
const tmp = () => mkdtempSync(join(tmpdir(), "membership-keyed-"))

const grant = (member: string, role: "member" | "editor" = "member", space = SPACE): RoleEvent => ({kind: "grant", space, member, role})
const revoke = (member: string, role: "member" | "editor" = "member", space = SPACE): RoleEvent => ({kind: "revoke", space, member, role})

function roleMessage(space: string, member: string, role: 0 | 1): Uint8Array {
	const field = (n: number, b: Buffer) => Buffer.concat([Buffer.from([(n << 3) | 2, b.length]), b])
	const parts = [field(1, Buffer.from(space, "hex")), field(2, Buffer.from(member, "hex"))]
	if (role !== 0) parts.push(Buffer.from([(3 << 3) | 0, role]))
	return Buffer.concat(parts)
}

/**
 * A model of the engine's statement state for the three intents the keyed path sends: `add` of a held id keeps its
 * active bit (idempotent), `set_active` of an unseen id is admitted (K1) and recorded.
 */
class ModelStore {
	readonly entities = new Set<string>()
	readonly statements = new Map<string, {active: boolean; body: string | null}>()
	apply(zon: string): void {
		for (const line of zon.split("\n")) {
			const i = line.trim().replace(/,$/, "")
			let m = /^\.\{ \.create_entity = "([0-9a-f]{32})" \}$/.exec(i)
			if (m) {
				this.entities.add(m[1] as string)
				continue
			}
			m = /^\.\{ \.add = \.\{ \.id = "([0-9a-f]{32})", (.*) \} \}$/.exec(i)
			if (m) {
				const held = this.statements.get(m[1] as string)
				if (held?.body != null && held.body !== m[2]) throw new Error("add of a held id with a different body")
				this.statements.set(m[1] as string, {active: held ? held.active : true, body: m[2] as string})
				continue
			}
			m = /^\.\{ \.set_active = \.\{ \.statement = "([0-9a-f]{32})", \.active = (true|false) \} \}$/.exec(i)
			if (m) {
				const held = this.statements.get(m[1] as string)
				this.statements.set(m[1] as string, {active: m[2] === "true", body: held?.body ?? null})
				continue
			}
			if (i.startsWith(".{ .") && !i.startsWith(".{ .space =")) throw new Error(`unmodelled intent ${i}`)
		}
	}
	active(): string[] {
		return [...this.statements].filter(([, s]) => s.active && s.body).map(([id]) => id).sort()
	}
}

function fixture(opts: {foundFails?: string; model?: ModelStore} = {}) {
	const store = new Store(tmp())
	const systemSpaces = new SystemSpaces(store.dir)
	const applied: string[] = []
	const founds: string[] = []
	const published: string[] = []
	const deps: KeyedMembershipDeps = {
		db: {
			apply: async (zon: string) => {
				applied.push(zon)
				opts.model?.apply(zon)
				return {rc: 0, out: "", err: ""}
			},
			waitReady: async () => [],
			found: async (_s: string, nonce: string) => {
				founds.push(nonce)
				if (opts.foundFails) throw new Error(opts.foundFails)
				return GOV
			},
		},
		sealer: async () => SEALER,
		store,
		metrics: createMetrics(),
		beat: async () => {},
		sleep: async () => {},
		allowPut: true,
		systemSpaces,
		publishSystem: (t) => published.push(t),
		offsets: new ProposalOffsets(`${store.dir}/membership-keyed`),
	}
	return {store, systemSpaces, applied, founds, published, deps}
}

describe("keyedStatementId", () => {
	it("is recordStatementId(membership kind, [subject, role property, member entity])", () => {
		expect(keyedStatementId(SPACE, ALICE, "member")).toBe(
			recordStatementId(RECORD_KINDS.membership.id, [
				idPart(subjectEntity(SPACE)),
				idPart(ROLE_PROPERTY.member),
				idPart(cid("entity", ALICE)),
			]),
		)
	})
	it("separates role, member and Space, and never equals the legacy id", () => {
		const ids = [
			keyedStatementId(SPACE, ALICE, "member"),
			keyedStatementId(SPACE, ALICE, "editor"),
			keyedStatementId(SPACE, BOB, "member"),
			keyedStatementId(SPACE2, ALICE, "member"),
		]
		expect(new Set(ids).size).toBe(4)
		expect(ids[0]).not.toBe(statementId(SPACE, ALICE, "member"))
	})
	it("is pinned (a change re-keys every membership row)", () => {
		expect(keyedStatementId(SPACE, ALICE, "member")).toBe("baed08b9c03a41f69533d5c9bb3183a2")
	})
})

describe("keyedRoleIntents", () => {
	it("a grant is create_entity + add + set_active(true) on the keyed id; a revoke is set_active(false)", () => {
		const sid = keyedStatementId(SPACE, ALICE, "editor")
		const subject = subjectEntity(SPACE)
		expect(keyedRoleIntents(grant(ALICE, "editor"))).toEqual([
			`.{ .create_entity = "${subject}" }`,
			`.{ .add = .{ .id = "${sid}", .entity = "${subject}", .property = "${ROLE_PROPERTY.editor}", .value = .{ .reference = "${cid("entity", ALICE)}" } } }`,
			`.{ .set_active = .{ .statement = "${sid}", .active = true } }`,
		])
		expect(keyedRoleIntents(revoke(ALICE, "editor"))).toEqual([`.{ .set_active = .{ .statement = "${sid}", .active = false } }`])
	})
	it("never sends put", () => {
		for (const ev of [grant(ALICE), revoke(ALICE)]) expect(keyedRoleIntents(ev).some(isPutIntent)).toBe(false)
	})
})

describe("handleKeyedMembershipMessage", () => {
	it("founds the governance Space once, writes every Space's rows there, and keys only the first apply", async () => {
		const f = fixture()
		const h = (t: string) => ({"event-type": Buffer.from(t)})
		const m = (value: Uint8Array | null, headers?: Record<string, unknown>) => ({offset: "5", value, headers})
		expect(await handleKeyedMembershipMessage(f.deps, 0, m(roleMessage(SPACE, BOB, 1), h("ROLE_GRANTED")))).toBe("applied")
		expect(await handleKeyedMembershipMessage(f.deps, 0, m(roleMessage(SPACE2, BOB, 0), h("ROLE_REVOKED")))).toBe("applied")
		expect(await handleKeyedMembershipMessage(f.deps, 0, m(roleMessage(SPACE, BOB, 1), h("SPACE_LEFT")))).toBe("ignored")
		expect(await handleKeyedMembershipMessage(f.deps, 0, m(null))).toBe("ignored")
		expect(await handleKeyedMembershipMessage(f.deps, 0, m(Buffer.from([1]), h("ROLE_GRANTED")))).toBe("dead_letter")

		const nonce = systemSpaceNonce("governance")
		expect(f.founds).toEqual([nonce])
		expect(f.applied).toHaveLength(2)
		expect(f.applied[0]).toContain(`.space = "${GOV}", .key = "${nonce}"`)
		expect(f.applied[1]).toContain(`.space = "${GOV}", .intents`)
		expect(f.applied[1]).toContain(keyedStatementId(SPACE2, BOB, "editor"))

		// Recorded apart from the gaia Space map, keyed bit included, and published under its own key.
		expect(f.store.spaces).toEqual({})
		expect(JSON.parse(readFileSync(f.systemSpaces.path, "utf8"))).toEqual({governance: {geo: GOV, nonce, keyed: true}})
		expect(f.published).toHaveLength(1)
		expect(JSON.parse(f.published[0] as string)).toEqual({governance: {geo: GOV, nonce}})
		expect(new SystemSpaces(f.store.dir).records.governance).toEqual({geo: GOV, nonce, keyed: true})
	})
})

describe("replay", () => {
	const events: RoleEvent[] = [
		grant(ALICE),
		grant(BOB),
		grant(ALICE, "editor"),
		revoke(BOB),
		revoke(ALICE, "editor", SPACE2), // never granted: admitted, stays inactive
		grant(BOB, "member", SPACE2),
		revoke(ALICE),
		grant(ALICE),
		revoke(BOB, "member", SPACE2),
	]
	const expected = [keyedStatementId(SPACE, ALICE, "member"), keyedStatementId(SPACE, ALICE, "editor")].sort()

	const run = async (model: ModelStore, evs: RoleEvent[], f = fixture({model})) => {
		for (const [i, ev] of evs.entries()) {
			const r = await handleKeyedMembershipMessage(f.deps, 0, {
				offset: String(i),
				value: roleMessage(ev.space, ev.member, ev.role === "member" ? 1 : 0),
				headers: {"event-type": Buffer.from(ev.kind === "grant" ? "ROLE_GRANTED" : "ROLE_REVOKED")},
			})
			expect(r).toBe("applied")
		}
		return f
	}

	it("lands on the last event per key", async () => {
		const model = new ModelStore()
		await run(model, events)
		expect(model.active()).toEqual(expected)
	})

	it("converges when the topic is replayed from zero over a store that holds it (or any prefix of it)", async () => {
		for (let cut = 0; cut <= events.length; cut++) {
			const model = new ModelStore()
			const f = await run(model, events.slice(0, cut))
			await run(model, events, f)
			expect(model.active()).toEqual(expected)
		}
	})

	it("sends the same intents on a replay: nothing depends on what was applied before", async () => {
		const a = await run(new ModelStore(), events)
		const b = await run(new ModelStore(), [...events, ...events])
		const body = (zon: string) => zon.replace(/, \.key = "[0-9a-f]{32}"/, "")
		expect(b.applied.slice(events.length).map(body)).toEqual(a.applied.map(body))
	})

	it("a backfill over a live store and a redelivered stream still converge", async () => {
		const model = new ModelStore()
		const f = await run(model, events)
		const rows = [
			{space_id: SPACE, member_space_id: ALICE, role: "member" as const},
			{space_id: SPACE, member_space_id: ALICE, role: "editor" as const},
		]
		expect(await backfillMembershipKeyed(f.deps, rows)).toEqual({rows: 2, applied: 2, deadLetter: 0})
		await run(model, events.slice(-3), f)
		expect(model.active()).toEqual(expected)
	})
})

describe("system space founding", () => {
	it("dead-letters a row and stops founding when geo-db is at its space cap", async () => {
		const f = fixture({foundFails: "error: ResourceExhausted (space cap)"})
		const msg = {offset: "1", value: roleMessage(SPACE, ALICE, 1), headers: {"event-type": Buffer.from("ROLE_GRANTED")}}
		expect(await handleKeyedMembershipMessage(f.deps, 0, msg)).toBe("dead_letter")
		expect(await handleKeyedMembershipMessage(f.deps, 0, msg)).toBe("dead_letter")
		expect(f.founds).toHaveLength(1)
		expect(f.applied).toEqual([])
		expect(readFileSync(join(f.store.dir, "errors.jsonl"), "utf8")).toContain("space cap")
		await expect(ensureSystemSpace(f.deps, "governance")).rejects.toBeInstanceOf(SystemSpaceCapError)
	})

	it("rethrows any other founding failure, so the consumer retries the message", async () => {
		const f = fixture({foundFails: "connection refused"})
		await expect(ensureSystemSpace(f.deps, "votes")).rejects.toThrow(/connection refused/)
		expect(f.systemSpaces.records.votes).toMatchObject({geo: null, nonce: systemSpaceNonce("votes")})
		expect(f.systemSpaces.render()).toBe("{}\n")
	})
})

describe("put guard", () => {
	const PUT = `.{ .put = .{ .id = "${"1".repeat(32)}", .entity = "${"2".repeat(32)}", .property = "${"3".repeat(32)}", .value = .{ .text = "x" } } }`

	it("putIntent builds the client's form, with the id it requires", () => {
		const [id, entity, property] = ["1".repeat(32), "2".repeat(32), "3".repeat(32)]
		expect(putIntent({id, entity, property, value: `.{ .text = "x" }`})).toBe(PUT)
		expect(putIntent({id, entity, property, value: `.{ .text = "x" }`, language: "en"})).toContain(`, .language = "en" } }`)
		expect(isPutIntent(putIntent({id, entity, property, value: `.{ .text = "x" }`}))).toBe(true)
		expect(() => putIntent({id: "", entity, property, value: `.{ .text = "x" }`})).toThrow(/\.id/)
		expect(() => putIntent({id, entity: "x", property, value: `.{ .text = "x" }`})).toThrow(/\.entity/)
		expect(() => putIntent({id, entity, property, value: `"x"`})).toThrow(/ZON value/)
	})

	it("recognises the put tag only", () => {
		expect(isPutIntent(PUT)).toBe(true)
		expect(isPutIntent(`.{ .add = .{ .id = "x" } }`)).toBe(false)
		expect(isPutIntent(`.{ .set = .{ .entity = "e", .property = "p", .value = .{ .text = ".{ .put = " } } }`)).toBe(false)
	})

	it("refuses to send put unless allowed, before anything reaches geo-db", async () => {
		const f = fixture()
		const space = {geo: GOV, nonce: "n", keyed: true}
		const meta = {space: SPACE, partition: 0, offset: "1"}
		const off = {...f.deps, allowPut: undefined}
		await expect(applyIntents(off, space, meta, [PUT], 10, 0, () => {})).rejects.toThrow(/GEODB_RECORD_KEYS is off/)
		await expect(applyIntents({...f.deps, allowPut: false}, space, meta, [PUT], 10, 0, () => {})).rejects.toThrow()
		expect(f.applied).toEqual([])
		expect(() => assertPutAllowed({}, [`.{ .create_entity = "${"1".repeat(32)}" }`])).not.toThrow()
		await applyIntents(f.deps, space, meta, [PUT], 10, 0, () => {})
		expect(f.applied).toHaveLength(1)
	})
})

describe("keyed shadow compare", () => {
	it("reads every Space's rows from the governance Space under keyed ids", async () => {
		const f = fixture()
		await ensureSystemSpace(f.deps, "governance")
		const reads: [string, string][] = []
		const s = await compareMembership({
			pgMembers: async () => new Map([[SPACE, {member: [ALICE], editor: []}]]),
			reader: {
				reset: () => {},
				entityRows: async (geo: string, entity: string) => {
					reads.push([geo, entity])
					return [{prop: ROLE_PROPERTY.member, value: "v", statement: keyedStatementId(SPACE, ALICE, "member"), entity}]
				},
			},
			spaces: () => ({}),
			journalSpaces: () => [],
			metrics: createMetrics(),
			sleep: async () => {},
			target: keyedCompareTarget(f.systemSpaces),
		})
		expect(reads).toEqual([[GOV, subjectEntity(SPACE)]])
		expect(s).toMatchObject({compared: 2, mismatched: [], unmapped: 0, errors: 0})
	})

	it("counts every Space unmapped until the governance Space is founded", async () => {
		const f = fixture()
		const s = await compareMembership({
			pgMembers: async () => new Map([[SPACE, {member: [ALICE], editor: []}]]),
			reader: {reset: () => {}, entityRows: async () => []},
			spaces: () => ({[SPACE]: {geo: "a".repeat(32), nonce: "n", keyed: true}}),
			journalSpaces: () => [],
			metrics: createMetrics(),
			target: keyedCompareTarget(f.systemSpaces),
		})
		expect(s.unmapped).toBe(1)
	})
})

describe("system space publishing", () => {
	it("writes the ConfigMap under system-spaces.json only, leaving space-map.json alone", async () => {
		const puts: Record<string, string>[] = []
		const p = new SpaceMapPublisher(join(tmp(), "system-space-map.json"), async (d) => void puts.push(d), createMetrics(), SYSTEM_SPACE_MAP_KEY)
		p.publish("{}\n")
		await p.flush()
		expect(puts).toEqual([{"system-spaces.json": "{}\n"}])
	})
})

describe("config", () => {
	const base = {KAFKA_BROKER: "b", KAFKA_USERNAME: "u", KAFKA_PASSWORD: "p", KAFKA_SSL_CA_PEM: "c", GEODB_KEY_PATH: "k", GEODB_BINDING_PATH: "b"}
	it("GEODB_RECORD_KEYS defaults off, with its own membership group", () => {
		const c = loadConfig(base)
		expect(c.recordKeys).toBe(false)
		expect(c.membership.keyedGroupId).toBe("geodb-feeder-membership-keyed-v1-testnet")
		expect(c.membership.keyedGroupId).not.toBe(c.membership.groupId)
		expect(c.systemSpaceMapPath).toBe("/data/feeder/system-space-map.json")
		expect(loadConfig({...base, GEODB_RECORD_KEYS: "1"}).recordKeys).toBe(true)
	})
})
