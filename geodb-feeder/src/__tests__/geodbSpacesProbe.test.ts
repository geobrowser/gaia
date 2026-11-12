import {mkdtempSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {GeoDb, SPACES_PROBE_TIMEOUT_MS, type Runner} from "../geodb.js"

function makeDb(run: Runner) {
	return new GeoDb({runDir: mkdtempSync(join(tmpdir(), "geodb-probe-"))}, run, async () => {})
}

describe("GeoDb.spaces probe timeout", () => {
	it("kills a hung probe after a few seconds, not twenty", async () => {
		expect(SPACES_PROBE_TIMEOUT_MS).toBeLessThanOrEqual(3_000)
		let seen: number | undefined
		const db = makeDb(async (_args, opts) => {
			seen = opts?.timeoutMs
			return {rc: null, out: "", err: ""}
		})
		expect(await db.spaces()).toBeNull()
		expect(seen).toBe(SPACES_PROBE_TIMEOUT_MS)
	})

	it("still honours an explicit timeout", async () => {
		let seen: number | undefined
		const db = makeDb(async (_args, opts) => {
			seen = opts?.timeoutMs
			return {rc: null, out: "", err: ""}
		})
		await db.spaces(10_000)
		expect(seen).toBe(10_000)
	})

	it("waitReady retries a killed probe and returns once serve answers", async () => {
		let n = 0
		const db = makeDb(async () => (++n < 3 ? {rc: null, out: "", err: ""} : {rc: 0, out: "", err: ""}))
		expect(await db.waitReady()).toEqual([])
		expect(n).toBe(3)
	})
})
