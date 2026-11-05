import {describe, expect, it} from "vitest"
import {RECORD_KINDS, SYSTEM_FAMILIES, SYSTEM_SPACE_NONCE_KIND} from "../recordKinds.js"
import {systemSpaceNonce} from "../systemSpaces.js"

const V4 = /^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/

describe("record kinds", () => {
	it("are pinned: a changed kind is a second copy of its table under new ids", () => {
		expect(Object.fromEntries(Object.entries(RECORD_KINDS).map(([k, v]) => [k, [v.id, v.family]]))).toEqual({
			membership: ["561407a0d813435fb6ed474be1a96209", "governance"],
			subspace: ["de1a992f8c2840c5bd185fdfa6aa28e9", "governance"],
			proposalAction: ["22335b74e17a4c9e867d4ac2a32c3a4a", "governance"],
			spaceVotingSettings: ["7767f888a9a240b18275a7832363f4d9", "governance"],
			proposalVote: ["ca89a877762c4883a3640b5455a8f0e2", "votes"],
			userVote: ["1623234736184c0d9a0b69e2ffbc124d", "votes"],
		})
		expect(SYSTEM_SPACE_NONCE_KIND).toBe("0b3f6634fbe84dbd8c0ecae3fce960b3")
	})

	it("are distinct UUID v4s in a known family", () => {
		const ids = [...Object.values(RECORD_KINDS).map((k) => k.id), SYSTEM_SPACE_NONCE_KIND]
		for (const id of ids) expect(id).toMatch(V4)
		expect(new Set(ids).size).toBe(ids.length)
		for (const k of Object.values(RECORD_KINDS)) expect(SYSTEM_FAMILIES).toContain(k.family)
	})
})

describe("system space nonces", () => {
	it("are pinned per family (the geo-db Space id is foundingSpaceId(sealer, nonce))", () => {
		expect(Object.fromEntries(SYSTEM_FAMILIES.map((f) => [f, systemSpaceNonce(f)]))).toEqual({
			governance: "305fbd2b17df43d48d79d806a01c1567",
			votes: "744a1a2a52d0432199372b395a4057b7",
			config: "be01e568dac84b488d279893b1133d33",
		})
	})
	it("are distinct v4s", () => {
		const n = SYSTEM_FAMILIES.map(systemSpaceNonce)
		for (const id of n) expect(id).toMatch(V4)
		expect(new Set(n).size).toBe(n.length)
	})
})
