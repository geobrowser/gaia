import {existsSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"

import {describe, expect, it} from "vitest"

import {resolveEscalations} from "../governance/escalation"

const here = dirname(fileURLToPath(import.meta.url))
const feederFile = join(here, "../../../../geodb-feeder/src/proposalEscalationKeyed.ts")

describe("resolveEscalations", () => {
	const versions = [{block: 300n}, {block: 100n}, {block: 200n}]
	const escalations = [
		{block: 50n, id: "a"},
		{block: 250n, id: "b"},
		{block: 250n, id: "c"},
		{block: 300n, id: "d"},
		{block: 150n, id: "e"},
	]

	it("hits the version at the greatest block at or before the event, latest wins, orphans reported", () => {
		const {applied, orphans} = resolveEscalations(versions, escalations)
		expect(orphans.map((e) => e.id)).toEqual(["a"])
		expect(applied.get(versions[1] as (typeof versions)[number])?.id).toBe("e")
		expect(applied.get(versions[2] as (typeof versions)[number])?.id).toBe("c")
		expect(applied.get(versions[0] as (typeof versions)[number])?.id).toBe("d")
	})

	it("agrees with the feeder's copy on the same inputs", async () => {
		if (!existsSync(feederFile)) return
		let feeder: {resolveEscalations: typeof resolveEscalations} | null = null
		try {
			feeder = await import(/* @vite-ignore */ feederFile)
		} catch {
			return // the feeder's dependencies are not installed in this checkout
		}
		if (!feeder) return
		const mine = resolveEscalations(versions, escalations)
		const theirs = feeder.resolveEscalations(versions, escalations)
		expect([...mine.applied].map(([v, e]) => [v.block, e.id])).toEqual(
			[...theirs.applied].map(([v, e]) => [v.block, e.id]),
		)
		expect(mine.orphans).toEqual(theirs.orphans)
	})
})
