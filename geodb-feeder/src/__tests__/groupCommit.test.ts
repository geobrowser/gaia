import {describe, expect, it} from "vitest"
import {createGroupCommitter} from "../groupCommit.js"

const setup = () => {
	let t = 1_000_000
	const commits: [number, string][] = []
	const c = createGroupCommitter(
		async (partition, next) => {
			commits.push([partition, next])
		},
		5000,
		() => t,
	)
	return {c, commits, advance: (ms: number) => (t += ms)}
}

describe("group commits", () => {
	it("commits a partition's first finished message, then throttles it", async () => {
		const {c, commits, advance} = setup()
		await c.done(0, 10)
		await c.done(0, 11)
		advance(5001)
		await c.done(0, 12)
		expect(commits).toEqual([
			[0, "11"],
			[0, "13"],
		])
	})

	it("flush commits what a batch left pending, so a caught-up consumer shows no group lag", async () => {
		const {c, commits} = setup()
		for (let o = 0; o <= 2780; o++) await c.done(0, o)
		await c.flush()
		expect(commits.at(-1)).toEqual([0, "2781"])
		await c.flush()
		expect(commits).toHaveLength(2)
	})

	it("throttles each partition on its own", async () => {
		const {c, commits} = setup()
		await c.done(1, 5)
		await c.done(0, 7)
		expect(commits).toEqual([
			[1, "6"],
			[0, "8"],
		])
	})
})
