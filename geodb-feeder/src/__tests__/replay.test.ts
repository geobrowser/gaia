import {describe, expect, it} from "vitest"
import {Series} from "../metrics.js"
import {type Pools, checkTarget, loadPools, loadPoolsFromApi, mappedSpaces, parsePairSource, parseRate, pick, replayOne, zipfBySpace} from "../replay.js"

const S1 = "41e851610e13a19441c4d980f2f2ce6b"
const S2 = "a19c345ab9866679b001d7d2138d88a1"
const E1 = "073ff16be7fe4591a984c325d629d92d"

describe("replay guards", () => {
	it("only calls Service api on port 3000 over cluster http", () => {
		expect(checkTarget("http://api:3000").origin).toBe("http://api:3000")
		expect(checkTarget("http://api.gaia.svc.cluster.local:3000/").hostname).toBe("api.gaia.svc.cluster.local")
		for (const bad of [
			"https://api:3000",
			"http://api:80",
			"http://api.geobrowser.io:3000",
			"https://api-testnet.geobrowser.io",
			"http://api.other:3000",
			"http://u:p@api:3000",
		]) {
			expect(() => checkTarget(bad), bad).toThrow()
		}
	})

	it("rate defaults to 0.5 and never exceeds 1", () => {
		expect(parseRate(undefined)).toBe(0.5)
		expect(parseRate("1")).toBe(1)
		expect(parseRate("0.2")).toBe(0.2)
		for (const bad of ["0", "-1", "1.01", "5", "x"]) expect(() => parseRate(bad), bad).toThrow()
	})

	it("reads the mapped spaces from the published map", () => {
		expect(mappedSpaces(JSON.stringify({[S1]: {geo: "g"}, [S2]: {}}))).toEqual(new Set([S1]))
		expect(mappedSpaces(JSON.stringify({[S1]: {geo: "g", loaded: false}, [S2]: {geo: "h"}}))).toEqual(new Set([S2]))
	})
})

describe("pool weighting", () => {
	it("ranks top pairs by score within each Space", () => {
		const ranked = zipfBySpace([
			{space_id: "a", score: "0.2", id: 1},
			{space_id: "b", score: "1", id: 2},
			{space_id: "a", score: "0.9", id: 3},
		])
		expect(ranked.map(([r, rank]) => [r.id, rank])).toEqual([
			[3, 1],
			[1, 2],
			[2, 1],
		])
	})

	it("splits picks 50/30/20 across pools and favours higher-ranked pairs", () => {
		const pools: Pools = {
			top: [
				{entity: "t1", space: S1, weight: 1},
				{entity: "t2", space: S1, weight: 0.5},
			],
			recent: [{entity: "r", space: S1, weight: 1}],
			random: [{entity: "x", space: S1, weight: 1}],
		}
		let seed = 1
		const rand = () => {
			seed = (seed * 16807) % 2147483647
			return seed / 2147483647
		}
		const counts: Record<string, number> = {}
		for (let i = 0; i < 20_000; i++) {
			const p = pick(pools, rand)
			if (p) counts[p.entity] = (counts[p.entity] ?? 0) + 1
		}
		const share = (k: string) => (counts[k] ?? 0) / 20_000
		expect(share("t1") + share("t2")).toBeCloseTo(0.5, 1)
		expect(share("r")).toBeCloseTo(0.3, 1)
		expect(share("x")).toBeCloseTo(0.2, 1)
		expect(share("t1")).toBeGreaterThan(share("t2") * 1.5)
	})

	it("passes an empty pool's share on, and picks nothing from nothing", () => {
		expect(pick({top: [], recent: [], random: [{entity: "x", space: S1, weight: 1}]})?.entity).toBe("x")
		expect(pick({top: [], recent: [], random: []})).toBeNull()
	})

	it("loads pools with read-only queries only", async () => {
		const seen: string[] = []
		const pg = {
			read: async <R>(text: string): Promise<R[]> => {
				seen.push(text)
				if (text.includes("local_scores")) return [{entity_id: E1, space_id: S1, score: "1"}] as R[]
				return [{entity_id: E1, space_id: S2}] as R[]
			},
		}
		const pools = await loadPools(pg, [S1, S2])
		expect(pools.top).toEqual([{entity: E1, space: S1, weight: 1}])
		expect(pools.recent).toHaveLength(1)
		expect(seen.every((q) => /^\s*SELECT/i.test(q))).toBe(true)
		expect(await loadPools(pg, [])).toEqual({top: [], recent: [], random: []})
	})
})

describe("replayOne", () => {
	const target = checkTarget("http://api:3000")
	const counters = () => ({requests: new Series("r", "r", "counter")})

	it("calls the card route in cluster and counts the result", async () => {
		const urls: string[] = []
		const c = counters()
		const r = await replayOne(target, {entity: E1, space: S1, weight: 1}, new Set([S1]), c, async (url) => {
			urls.push(url)
			return {status: 200, text: async () => "{}"}
		})
		expect(r).toBe("ok")
		expect(urls).toEqual([`http://api:3000/geodb/entities/${E1}/card?spaceId=${S1}`])
		expect(c.requests.render()).toContain('r{result="ok"} 1')
	})

	it("skips a Space mapped out since the pools were built, without a request", async () => {
		let called = false
		const r = await replayOne(target, {entity: E1, space: S2, weight: 1}, new Set([S1]), counters(), async () => {
			called = true
			return {status: 200, text: async () => ""}
		})
		expect(r).toBe("skipped_unmapped")
		expect(called).toBe(false)
	})

	it("counts http errors and failures, and never throws", async () => {
		const pair = {entity: E1, space: S1, weight: 1}
		expect(await replayOne(target, pair, new Set([S1]), counters(), async () => ({status: 500, text: async () => ""}))).toBe("http_error")
		expect(
			await replayOne(target, pair, new Set([S1]), counters(), async () => {
				throw new Error("ECONNREFUSED")
			}),
		).toBe("error")
	})
})

describe("api pair source", () => {
	const target = checkTarget("http://api:3000")
	const E2 = "b".repeat(32)
	const reply = (body: unknown, status = 200) => async () => ({status, text: async () => JSON.stringify(body)})

	it("defaults to postgres and refuses an unknown source", () => {
		expect(parsePairSource(undefined)).toBe("postgres")
		expect(parsePairSource("")).toBe("postgres")
		expect(parsePairSource("API")).toBe("api")
		expect(() => parsePairSource("geodb")).toThrow()
	})

	it("posts the mapped Spaces to Service api and builds the same pools", async () => {
		const calls: {url: string; method?: string; body?: string}[] = []
		const pools = await loadPoolsFromApi(
			async (url, init) => {
				calls.push({url, method: init.method, body: init.body})
				return {
					status: 200,
					text: async () =>
						JSON.stringify({
							top: [
								{entity: E1, space: S1, score: "0.2"},
								{entity: E2, space: S1, score: "0.9"},
								{entity: E2, space: S2, score: "1"},
							],
							recent: [{entity: E1, space: S1}],
							random: [{entity: E2, space: S2}],
						}),
				}
			},
			target,
			[S1],
		)
		expect(calls).toEqual([{url: "http://api:3000/geodb/replay-pairs", method: "POST", body: JSON.stringify({spaces: [S1], perSpace: 50, limit: 1500})}])
		// S2 is not a requested Space, so its row is dropped; within S1 the higher score ranks first.
		expect(pools.top).toEqual([
			{entity: E2, space: S1, weight: 1},
			{entity: E1, space: S1, weight: 0.5},
		])
		expect(pools.recent).toEqual([{entity: E1, space: S1, weight: 1}])
		expect(pools.random).toEqual([])
	})

	it("sends the token in x-internal-token, and none when no token is given", async () => {
		const seen: (Record<string, string> | undefined)[] = []
		const f = async (_url: string, init: {headers?: Record<string, string>}) => {
			seen.push(init.headers)
			return {status: 200, text: async () => JSON.stringify({top: [], recent: [], random: []})}
		}
		await loadPoolsFromApi(f, target, [S1], 50, 1500, 90_000, "tok-123")
		await loadPoolsFromApi(f, target, [S1])
		expect(seen[0]).toEqual({"content-type": "application/json", "x-internal-token": "tok-123"})
		expect(seen[1]).toEqual({"content-type": "application/json"})
	})

	it("drops malformed rows, and throws on a non-2xx or a malformed answer", async () => {
		const bad = {top: [{entity: "x", space: S1, score: "1"}, {entity: E1, space: S1, score: "nope"}], recent: [{entity: E1}], random: []}
		expect(await loadPoolsFromApi(reply(bad), target, [S1])).toEqual({top: [], recent: [], random: []})
		await expect(loadPoolsFromApi(reply({}, 503), target, [S1])).rejects.toThrow("503")
		await expect(loadPoolsFromApi(reply({top: []}), target, [S1])).rejects.toThrow("recent")
	})

	it("asks nothing when no Space is mapped", async () => {
		let called = false
		const pools = await loadPoolsFromApi(async () => {
			called = true
			return {status: 200, text: async () => "{}"}
		}, target, [])
		expect(pools).toEqual({top: [], recent: [], random: []})
		expect(called).toBe(false)
	})
})
