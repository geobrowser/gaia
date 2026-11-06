import {spawn as nodeSpawn} from "node:child_process"
import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {GeoDbError} from "../client"
import {createDerivedReader, parseDerivedGet} from "../derived"

const GROUP = "0".repeat(32)
const KEY = "ab".repeat(16)

/** A fake `geodb-client derived pipe`: a node script printing `ready`, then answering each line with a frame. */
function fake(body: string, timeoutMs = 2000) {
	const dir = mkdtempSync(join(tmpdir(), "geodb-derived-"))
	const script = join(dir, "fake.mjs")
	writeFileSync(
		script,
		`
const frame = (kind, text) => process.stdout.write(kind + Buffer.byteLength(text) + "\\n" + text)
let buffer = ""
const onLine = (line) => {
${body}
}
process.stdout.write("ready\\n")
process.stdin.setEncoding("utf8")
process.stdin.on("data", (chunk) => {
	buffer += chunk
	for (let i; (i = buffer.indexOf("\\n")) >= 0; ) {
		const line = buffer.slice(0, i)
		buffer = buffer.slice(i + 1)
		onLine(line)
	}
})
`,
	)
	let starts = 0
	const reader = createDerivedReader(
		{clientBin: process.execPath, keyPath: "k", bindingPath: "b", timeoutMs},
		{
			spawn: () => {
				starts++
				return nodeSpawn(process.execPath, [script], {stdio: ["pipe", "pipe", "pipe"]}) as never
			},
		},
	)
	return {reader, starts: () => starts}
}

describe("parseDerivedGet", () => {
	it("reads a row and none", () => {
		const row = parseDerivedGet(`${GROUP}\t0.75\t0102ff\n`)
		expect(row?.group).toBe(GROUP)
		expect(row?.score).toBe(0.75)
		expect([...(row?.value ?? [])]).toEqual([1, 2, 255])
		expect(parseDerivedGet("none\n")).toBeNull()
	})
	it("refuses a malformed answer", () => {
		expect(() => parseDerivedGet("what")).toThrow(GeoDbError)
		expect(() => parseDerivedGet(`${GROUP}\tNaN\t00`)).toThrow(GeoDbError)
		expect(() => parseDerivedGet(`${GROUP}\t1\t0`)).toThrow(GeoDbError)
	})
})

describe("createDerivedReader", () => {
	it("sends a get line and parses the row, reusing one worker", async () => {
		const {reader, starts} = fake(`
			if (line === "get\\t5\\thex:${KEY}") frame("+", "${GROUP}\\t0.5\\t01")
			else frame("+", "none\\n")`)
		const row = await reader.get(5, KEY)
		expect(row?.score).toBe(0.5)
		expect(await reader.get(5, "cd".repeat(16))).toBeNull()
		expect(starts()).toBe(1)
		reader.close()
	})

	it("answers queued requests in order", async () => {
		const {reader} = fake(`frame("+", "${GROUP}\\t" + line.length + "\\t00")`)
		const rows = await Promise.all([reader.get(5, "aa"), reader.get(5, "aaaa"), reader.get(1, "aaaaaa")])
		expect(rows.map((r) => r?.score)).toEqual([
			`get\t5\thex:aa`.length,
			`get\t5\thex:aaaa`.length,
			`get\t1\thex:aaaaaa`.length,
		])
		reader.close()
	})

	it("rejects a refusal, then recovers", async () => {
		const {reader, starts} = fake(`
			if (line.includes("hex:ee")) frame("-", "refused: no grant")
			else frame("+", "none\\n")`)
		await expect(reader.get(5, "ee")).rejects.toMatchObject({reason: "client_failed"})
		await expect(reader.get(5, "aa")).resolves.toBeNull()
		expect(starts()).toBe(2)
		reader.close()
	})

	it("times out a silent worker and kills it", async () => {
		const {reader, starts} = fake(`if (line.includes("hex:ee")) {} else frame("+", "none\\n")`, 150)
		await expect(reader.get(5, "ee")).rejects.toMatchObject({reason: "timeout"})
		await expect(reader.get(5, "aa")).resolves.toBeNull()
		expect(starts()).toBe(2)
		reader.close()
	})

	it("rejects a bad request without starting a worker", async () => {
		const {reader, starts} = fake(`frame("+", "none\\n")`)
		await expect(reader.get(5, "not hex")).rejects.toMatchObject({reason: "bad_request"})
		await expect(reader.get(-1, "aa")).rejects.toMatchObject({reason: "bad_request"})
		expect(starts()).toBe(0)
	})

	it("fails when the worker does not print ready, and backs off", async () => {
		const reader = createDerivedReader(
			{clientBin: process.execPath, keyPath: "k", bindingPath: "b", timeoutMs: 1000},
			{
				spawn: () =>
					nodeSpawn(process.execPath, ["-e", "process.stdout.write('error: no binding\\n')"], {
						stdio: ["pipe", "pipe", "pipe"],
					}) as never,
			},
		)
		await expect(reader.get(5, "aa")).rejects.toMatchObject({reason: "client_failed"})
		await expect(reader.get(5, "aa")).rejects.toMatchObject({reason: "client_failed"})
		reader.close()
	})
})
