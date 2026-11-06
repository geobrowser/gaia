import {describe, expect, it} from "vitest"
import {RECORD_COLUMNS, RECORD_KINDS, SYSTEM_FAMILIES, SYSTEM_SPACE_NONCE_KIND} from "../recordKinds.js"
import {systemSpaceNonce} from "../systemSpaces.js"

const V4 = /^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/

describe("record kinds", () => {
	it("are pinned: a changed kind is a second copy of its table under new ids", () => {
		expect(Object.fromEntries(Object.entries(RECORD_KINDS).map(([k, v]) => [k, [v.id, v.family]]))).toEqual({
			membership: ["561407a0d813435fb6ed474be1a96209", "governance"],
			subspace: ["de1a992f8c2840c5bd185fdfa6aa28e9", "governance"],
			subspaceTopic: ["e90b36c75cbf4239a6add0ae26dd939a", "governance"],
			proposalAction: ["22335b74e17a4c9e867d4ac2a32c3a4a", "governance"],
			proposalVersion: ["6863abef6664450eb135a182c6ff1715", "governance"],
			proposalEscalation: ["69352153d76946afa5d926c13c3613e9", "governance"],
			spaceVotingSettings: ["7767f888a9a240b18275a7832363f4d9", "governance"],
			proposalStatus: ["cf398f45b2c74002bb13e5416fbc7027", "governance"],
			rankingBlock: ["ce9b2464e65c450a975d3a427baaf4a2", "votes"],
			ranking: ["ff8283aec584455a80198d6d80f0f4fc", "votes"],
			rankingItem: ["f8ad2041f5a34f4580a1ba3ca426730f", "votes"],
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
			spaceVotingSettings: {
				space: "15ae9fc7333d4d72aeb372321e58db00",
				partial: "a97262c9981f4663a20b4208cb2838d2",
				universal: "4cf81d6cc92b4e0e91f2cb8139d736a2",
				flat: "fcc01cfa9bee4781940a6be21fa67caa",
				quorum: "2cf27f7101af401d8e6de528d03b05d7",
				duration: "4e371d0baf1d46d7a506491c192016ae",
				disableFastPath: "52570d2d1e95478ab9d31d811371a7dd",
				grace: "198ba2f858844b668a6126d5bc8a069e",
				updatedAt: "c23df2162f6b4b7b8bc41d740afc0f19",
				updatedAtBlock: "8ba30f5e13da49699c351e1475c86f7b",
			},
			rankingBlock: {
				block: "30a0bcf61226497ea1a9fbdb05be369e",
				space: "723da53041a149e8bee03243676aec1a",
				name: "01c6cd80b4944f1eac79a2e069618dea",
				filter: "34f4cb764c2d4c88a40ad7553254fcae",
				startDate: "a522112064024103937ca4a9a8347e0b",
				endDate: "a78b82b1e9384e409f367720aa0a9110",
				restriction: "02a37e407aed4aa09dd50080d8a86404",
				rankingType: "ff6fd916831643b2a87ea23f13254d11",
				submissionFrequency: "ead65e7abfe64037aaef51050f4f2cc7",
			},
			ranking: {
				ranking: "5afd9abadca04814bb03eda81d951d22",
				space: "c84de538157b4562be0352add400ee2f",
				block: "e702f42b27a94659a38f3ee7f8aef0b8",
				rankType: "b943c785e30e4fa5a2f4c8886b4104de",
				submittedAt: "883ddfb02814461881f7ed2fdf0c6be5",
				updatedAtBlock: "e2a7000398d848a7a5e541f63fd6c1a1",
				updateIndex: "529e8bb7b260483eb78cd68aa221a8a5",
				author: "8acc92223b3643b39ceb4184c99d192a",
			},
			rankingItem: {
				ranking: "bc2d175676b04b5386b66ea4a3d6ffb3",
				entity: "3b05db08120843459c70cde796f14b68",
				space: "565168847c344611964809b17abcf7a8",
				position: "d4ea2f12989646ce9aaec4f5aad414a2",
				weight: "4ff16dd13cec48918186b04e30469eaf",
				generation: "c37546c6d73b41f5aabb0fdd96fa16cb",
			},
			proposalStatus: {
				proposal: "4f742a5f821a469c9c0be0b352ae5b67",
				executedAt: "ed3855a3c41b4f749651bf049dabac0b",
				unexecutableAt: "72179d56e3dc478d9c9ab2159e4b40ef",
			},
			proposalEscalation: {
				proposal: "49c19599b0694221841396f0db5f9995",
				block: "89cbe515c863439dbf2a7686c676c90a",
				votingMode: "e3f7ba26068849658179f6d443b0a064",
				startTime: "0c2fcb7931a34133a8a4258ae111f20e",
				endTime: "31d689b4544b4fcd91af181db291a4d8",
				quorum: "43a88c623ad243cebadbff37b0e6d567",
				partial: "4e2cf0e3c88d4b998b9a4932469f702e",
				universal: "d4124abe7be548548d39f67383669975",
				flat: "473d72f8ac84495284b3623afef491a0",
				executeBy: "7ef74c02759c4e658a3a73e18f8f0733",
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
			subspace: {verified: "2c527e632f3645cdbbc2c2a9806dfc05", related: "ba99f0b8bf4646ed9cdb404da918e5e1"},
			subspaceTopic: {topic: "90ee068ad2d44a8b95cf939d2d1ceabb"},
			proposalVersion: {
				proposal: "c42be9e340d14566b2b9f084004c748b",
				block: "bccb84d7017849b0babfa7444789068f",
				votingMode: "1ec5fb72263443668b4880d09670cb56",
				startTime: "5a806f60759e4dd08a39c771d7586f53",
				endTime: "72d87497d7ed49db81e31675a31c5da7",
				quorum: "54000906476b41b7a0ca655f043b1a8d",
				partial: "e4e76529227940f08bd1ca1ab884fe31",
				universal: "eecb13aaa70c44509070df7550436103",
				flat: "2e641ff402734c6b80201641f3969fe2",
				executeBy: "ca75ecfb5ab24196998de1b3bb31eb6f",
				name: "a76bbfa24cfa43708b89326ae5f72e18",
				createdAt: "6c74f4f248d249368e1002b4c393a59e",
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
