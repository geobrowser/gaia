import {mkdtempSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {cid} from "../ids.js"
import {createMetrics} from "../metrics.js"
import {
	applyProposal,
	handleProposalMessage,
	parseProposalCreated,
	ProposalOffsets,
	proposalRows,
	systemRelationId,
	uuidV5,
	type ProposalDeps,
} from "../proposals.js"
import {Store} from "../state.js"

const SPACE = "a19c345ab9866679b001d7d2138d88a1"
const PROPOSAL = "b29c345ab9866679b001d7d2138d88a2"
const GEO = "daf0bc83ca744e1a9ee0377967846846"
const tmp = () => mkdtempSync(join(tmpdir(), "proposals-"))
const bytes = (h: string) => Buffer.from(h, "hex")
const field = (n: number, b: Buffer) => Buffer.concat([Buffer.from([(n << 3) | 2, b.length]), b])

/** HermesProposalCreated: space_id = 1, proposal_id = 3, voting_mode = 4 (omitted when FAST), meta = 10 { block_number = 3 }. */
function message(mode: 0 | 1, block = 300): Buffer {
	const parts = [field(1, bytes(SPACE)), field(3, bytes(PROPOSAL))]
	if (mode !== 0) parts.push(Buffer.from([(4 << 3) | 0, mode]))
	parts.push(field(10, Buffer.from([(3 << 3) | 0, block & 0x7f | 0x80, block >> 7])))
	return Buffer.concat(parts)
}

function fixture(founded = true) {
	const store = new Store(tmp())
	if (founded) store.spaces[SPACE] = {geo: GEO} as never
	const applied: string[] = []
	const deps: ProposalDeps = {
		db: {
			apply: async (zon: string) => {
				applied.push(zon)
				return {rc: 0, out: "", err: ""}
			},
			waitReady: async () => [],
			found: async () => GEO,
		},
		sealer: async () => "5".repeat(64),
		store,
		metrics: createMetrics(),
		beat: async () => {},
		sleep: async () => {},
		publish: () => {},
	}
	return {store, applied, deps}
}

describe("ids", () => {
	it("uuidV5 and the system relation ids equal Rust's Uuid::new_v5 (checked against Python uuid5)", () => {
		expect(systemRelationId("System", "a19c345ab9866679b001d7d2138d88a1")).toBe("15d4cd883c755c699ba3098c4fd7a5ae")
		expect(systemRelationId("Proposal", "a19c345ab9866679b001d7d2138d88a1")).toBe("a7c6febc00eb56828f239d784dc3a9cc")
		expect(uuidV5("ae1e004d-b125-57d8-b3bf-e85a2484f129", "x")).toMatch(/^[0-9a-f]{12}5[0-9a-f]{3}[89ab]/)
	})
})

describe("parseProposalCreated", () => {
	it("reads space, proposal, voting mode and block; ignores other event types; rejects a malformed message", () => {
		expect(parseProposalCreated("PROPOSAL_CREATED", message(0))).toEqual({space: SPACE, proposal: PROPOSAL, votingMode: 0, block: 300n})
		expect(parseProposalCreated("PROPOSAL_CREATED", message(1))?.votingMode).toBe(1)
		expect(parseProposalCreated("PROPOSAL_VOTED", message(0))).toBeNull()
		expect(parseProposalCreated(undefined, message(0))).toBeNull()
		expect(() => parseProposalCreated("PROPOSAL_CREATED", Buffer.from([0x0a, 0x01, 0x01]))).toThrow()
	})
})

describe("proposalRows", () => {
	it("is the five values and two system relations the kg-indexer writes", () => {
		const r = proposalRows({space: SPACE, proposal: PROPOSAL, votingMode: 1, block: 300n})
		expect(r.values.map((v) => v.text ?? v.integer)).toEqual([
			"1",
			"a19c345a-b986-6679-b001-d7d2138d88a1",
			"300",
			"Proposal b29c345a-b986-6679-b001-d7d2138d88a2",
			"System entity for proposal b29c345a-b986-6679-b001-d7d2138d88a2 in space a19c345a-b986-6679-b001-d7d2138d88a1",
		])
		expect(r.relations.map((x) => x.to_entity_id)).toEqual(["2ff7ea09-8b9e-50bc-9be7-8a0cafa268d0", "1cb9d5ba-c730-52a7-bc73-1f2c3ff5a330"])
	})
})

describe("applyProposal", () => {
	it("creates the entity with its values and relations once; a repeat is a no-op", async () => {
		const f = fixture()
		const ev = parseProposalCreated("PROPOSAL_CREATED", message(0)) as NonNullable<ReturnType<typeof parseProposalCreated>>
		expect(await applyProposal(f.deps, ev, {partition: 0, offset: "1"})).toBe("applied")
		expect(await applyProposal(f.deps, ev, {partition: 0, offset: "1"})).toBe("noop")
		expect(f.applied).toHaveLength(1)
		const zon = f.applied[0] as string
		expect(zon).toContain(`.create_entity = "${cid("entity", PROPOSAL)}"`)
		expect((zon.match(/\.add = /g) ?? []).length).toBe(2)
		expect((zon.match(/\.set = /g) ?? []).length).toBe(5)
	})
	it("skips a proposal in a Space geo-db has not founded, without founding it", async () => {
		const f = fixture(false)
		const ev = parseProposalCreated("PROPOSAL_CREATED", message(0)) as NonNullable<ReturnType<typeof parseProposalCreated>>
		expect(await applyProposal(f.deps, ev, {partition: 0, offset: "1"})).toBe("unmapped")
		expect(f.applied).toHaveLength(0)
	})
	it("a refused intent is dead-lettered and not journaled", async () => {
		const f = fixture()
		f.deps.db.apply = async () => ({rc: 1, out: "", err: "InvalidIntent"})
		const ev = parseProposalCreated("PROPOSAL_CREATED", message(0)) as NonNullable<ReturnType<typeof parseProposalCreated>>
		expect(await applyProposal(f.deps, ev, {partition: 0, offset: "9"})).toBe("dead_letter")
		expect(f.store.hasEntity(SPACE, cid("entity", PROPOSAL))).toBe(false)
	})
})

describe("handleProposalMessage", () => {
	const headers = (t: string) => ({"event-type": Buffer.from(t)})
	it("applies PROPOSAL_CREATED, ignores other events and empty values, dead-letters garbage", async () => {
		const f = fixture()
		expect(await handleProposalMessage(f.deps, 0, {offset: "1", value: message(0), headers: headers("PROPOSAL_CREATED")})).toBe("applied")
		expect(await handleProposalMessage(f.deps, 0, {offset: "2", value: message(0), headers: headers("PROPOSAL_VOTED")})).toBe("ignored")
		expect(await handleProposalMessage(f.deps, 0, {offset: "3", value: null})).toBe("ignored")
		expect(await handleProposalMessage(f.deps, 0, {offset: "4", value: Buffer.from([1]), headers: headers("PROPOSAL_CREATED")})).toBe("dead_letter")
	})
})

describe("ProposalOffsets", () => {
	it("persists offsets", () => {
		const dir = tmp()
		const a = new ProposalOffsets(dir)
		expect(a.hadOffsets).toBe(false)
		a.markDone(1, 7)
		const b = new ProposalOffsets(dir)
		expect(b.hadOffsets).toBe(true)
		expect(b.lastDone(1)).toBe(7)
		expect(b.lastDone(0)).toBeNull()
	})
})
