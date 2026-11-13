import {mkdtempSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {classify, GeoDb, NodeOpeningError, type Runner} from "../geodb.js"

const SPACE = "daf0bc83ca744e1a9ee0377967846846"
const REFUSED = {rc: 1, out: "refused: ResourceExhausted ResourceExhausted\n", err: "error: Refused\n"}

function makeDb(run: Runner) {
	return new GeoDb({runDir: mkdtempSync(join(tmpdir(), "geodb-found-"))}, run, async () => {})
}

describe("GeoDb.found tells a node still opening from the Space cap", () => {
	it("a node that does not answer `spaces` is opening: NodeOpeningError, never an exhausted failure", async () => {
		const calls: string[] = []
		const db = makeDb(async (args) => {
			calls.push(args[0] ?? "")
			return args[0] === "found" ? REFUSED : {rc: 1, out: "", err: "error: Refused\n"}
		})
		const e = await db.found("5".repeat(64), "nonce").catch((x: unknown) => x)
		expect(e).toBeInstanceOf(NodeOpeningError)
		expect(classify((e as Error).message)).not.toBe("exhausted")
		expect(calls).toEqual(["found", "spaces"])
	})

	it("an open node that opened between the two calls founds the Space on the second try", async () => {
		let founds = 0
		const db = makeDb(async (args) => {
			if (args[0] === "spaces") return {rc: 0, out: "", err: ""}
			return ++founds === 1 ? REFUSED : {rc: 0, out: `founded: ${SPACE}\n`, err: ""}
		})
		expect(await db.found("5".repeat(64), "nonce")).toBe(SPACE)
	})

	it("an open node that refuses twice is at its Space cap: the exhausted failure reaches the caller", async () => {
		let founds = 0
		const db = makeDb(async (args) => {
			if (args[0] === "spaces") return {rc: 0, out: "", err: ""}
			founds++
			return REFUSED
		})
		const e = await db.found("5".repeat(64), "nonce").catch((x: unknown) => x)
		expect(e).not.toBeInstanceOf(NodeOpeningError)
		expect(classify((e as Error).message)).toBe("exhausted")
		expect(founds).toBe(2)
	})

	it("any other failure is raised at once, with no probe", async () => {
		const calls: string[] = []
		const db = makeDb(async (args) => {
			calls.push(args[0] ?? "")
			return {rc: 1, out: "refused: DefiniteRefusal NodeNotPinned\n", err: "error: Refused\n"}
		})
		await expect(db.found("5".repeat(64), "nonce")).rejects.toThrow(/NodeNotPinned/)
		expect(calls).toEqual(["found"])
	})
})
