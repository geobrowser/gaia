import {describe, expect, it} from "vitest"
import {createReplayPairsRouter, MAX_REPLAY_SPACES, parseReplayPairsRequest, type ReplayPairs} from "../replayPairs"

const S1 = "41e851610e13a19441c4d980f2f2ce6b"
const S1_DASHED = "41e85161-0e13-a194-41c4-d980f2f2ce6b"
const E1 = "073ff16be7fe4591a984c325d629d92d"

describe("parseReplayPairsRequest", () => {
	it("normalizes, dedupes and clamps", () => {
		expect(parseReplayPairsRequest({spaces: [S1_DASHED, S1], perSpace: 5000, limit: 0})).toEqual({
			spaces: [S1],
			perSpace: 100,
			limit: 1500,
		})
		expect(parseReplayPairsRequest({spaces: []})?.spaces).toEqual([])
	})
	it("rejects bad bodies", () => {
		for (const bad of [null, "x", {}, {spaces: "a"}, {spaces: ["nope"]}, {spaces: [1]}]) {
			expect(parseReplayPairsRequest(bad), JSON.stringify(bad)).toBeNull()
		}
		expect(parseReplayPairsRequest({spaces: Array(MAX_REPLAY_SPACES + 1).fill(S1)})).toBeNull()
	})
})

describe("replay-pairs route", () => {
	const answer: ReplayPairs = {top: [{entity: E1, space: S1, score: "1"}], recent: [], random: []}
	const post = (router: ReturnType<typeof createReplayPairsRouter>, body: unknown) =>
		router.request("/replay-pairs", {
			method: "POST",
			body: JSON.stringify(body),
			headers: {"content-type": "application/json"},
		})

	it("answers 400 on a bad body and never reads", async () => {
		let reads = 0
		const router = createReplayPairsRouter({} as never, {
			read: async () => {
				reads++
				return answer
			},
		})
		expect((await post(router, {spaces: ["x"]})).status).toBe(400)
		expect((await router.request("/replay-pairs", {method: "POST", body: "{"})).status).toBe(400)
		expect(reads).toBe(0)
	})

	it("caches an answer for cacheMs, then reads again", async () => {
		let reads = 0
		let t = 0
		const router = createReplayPairsRouter({} as never, {
			cacheMs: 1000,
			now: () => t,
			read: async () => {
				reads++
				return answer
			},
		})
		const res = await post(router, {spaces: [S1]})
		expect(await res.json()).toEqual(answer)
		await post(router, {spaces: [S1_DASHED]})
		expect(reads).toBe(1)
		t = 1001
		await post(router, {spaces: [S1]})
		expect(reads).toBe(2)
	})

	it("answers 503 on a failed read and does not cache the failure", async () => {
		let reads = 0
		const router = createReplayPairsRouter({} as never, {
			read: async () => {
				reads++
				throw new Error("boom")
			},
		})
		expect((await post(router, {spaces: [S1]})).status).toBe(503)
		expect((await post(router, {spaces: [S1]})).status).toBe(503)
		expect(reads).toBe(2)
	})
})
