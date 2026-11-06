import {describe, expect, it} from "vitest"
import {RECORD_COLUMNS, RECORD_KINDS, SYSTEM_FAMILIES, SYSTEM_SPACE_NONCE_KIND} from "../recordKinds.js"
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

describe("record columns", () => {
	it("are pinned: a changed column id moves every keyed row's statements", () => {
		expect(RECORD_COLUMNS).toEqual({
			userVote: {
				user: "4bd6d3068a204ff6a62bb084263045bc",
				object: "46e83cb03e0c46a4a874d7deeb5889dd",
				objectType: "e2f5df668d07476ba3f1721c885bd53e",
				space: "59217d4ba84c4fdcbdc3ff6fb3cb83fa",
				kind: "4f65ca0db5874a519d8068671a0222c5",
				direction: "8f3588d7a11c4e7eb86b61015670b491",
				votedAt: "6e088d8604924b05b09e3ebeceebf5db",
			},
			proposalVote: {
				proposal: "c020ebd69bbb4bec8c83db30765aa91b",
				version: "da1d8b5c47ee499797e729ae90938086",
				voter: "e033f679eebb40aeaec8c66f9a1d6af9",
				space: "49a4ffb094684e10843bbc1e4c6a71f8",
				vote: "328dc69e2c964982a35fe4d3aa79e31a",
				createdAt: "98603a29c209486b89a63ac207cf9e2f",
				createdAtBlock: "35243b2b183648059546576e381806c1",
			},
			proposalAction: {
				proposal: "235abc0092a248d69e2dbb5c71fd0209",
				block: "a93c92d0a2f442e9852057064567e012",
				index: "81b028b2bb7f4f9e85e5689a2f1826af",
				actionType: "002e8db762e0401386b53478b329013e",
				target: "afc49d0ddf9c4affaae9ff963338d7c4",
				contentUri: "a267fc081a4c457abfda85b8fc024d92",
				metadata: "5ee665ce733b46ffa8d6697c1b2cedcb",
				contentId: "268e38bcf4d247138d7dbd154873391c",
				quorum: "83b46fb278414103866bcba416f7c2d2",
				duration: "bbf27d7c47634001b35d46ace45371ff",
				partial: "c13d280d658247d0859a3c0ce0367da4",
				universal: "603b264f392c4aa5be3d9819bc04b106",
				flat: "d0d34304079149e78157e70d6b3722d4",
				disableFastPath: "4a13aa7d7f55476fb307b2d19b14dfe7",
				grace: "401c3bec5a414703a5b6b51ce8e18e13",
			},
		})
	})
	it("are distinct v4s, distinct from every kind", () => {
		const cols = Object.values(RECORD_COLUMNS).flatMap((c) => Object.values(c))
		for (const id of cols) expect(id).toMatch(V4)
		const all = [...cols, ...Object.values(RECORD_KINDS).map((k) => k.id), SYSTEM_SPACE_NONCE_KIND]
		expect(new Set(all).size).toBe(all.length)
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
