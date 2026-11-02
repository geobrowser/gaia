import {spawn as nodeSpawn} from "node:child_process"
import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {cardDocument, createPipeGeoDbClient, createSpaceMap, GeoDbError} from "../client"
import type {GeoDbCardConfig} from "../config"

const ENTITY = "073ff16be7fe4591a984c325d629d92d"
const SPACE = "22222222222242228222222222222222"
const PROP = "98012398829d48b083738f2589675894"

/** A fake `geodb-client pipe`: a node script speaking the frame protocol, run through the client's spawn seam. */
function fakePipe(body: string, options: {timeoutMs?: number; poolSize?: number} = {}) {
	const dir = mkdtempSync(join(tmpdir(), "geodb-pipe-"))
	const script = join(dir, "fake.mjs")
	writeFileSync(
		script,
		`
const frame = (kind, text) => process.stdout.write(kind + Buffer.byteLength(text) + "\\n" + text)
const lines = (onLine) => {
	let buffer = ""
	process.stdin.setEncoding("utf8")
	process.stdin.on("data", (chunk) => {
		buffer += chunk
		for (let i; (i = buffer.indexOf("\\n")) >= 0; ) {
			const line = buffer.slice(0, i)
			buffer = buffer.slice(i + 1)
			onLine(line)
		}
	})
}
${body}
`,
	)
	const config: GeoDbCardConfig = {
		mode: "on",
		clientBin: process.execPath,
		keyPath: "k",
		bindingPath: "b",
		spaceMapPath: "",
		timeoutMs: options.timeoutMs ?? 2000,
		poolSize: options.poolSize ?? 2,
	}
	let starts = 0
	const client = createPipeGeoDbClient(config, {
		spawn: () => {
			starts++
			return nodeSpawn(process.execPath, [script, String(starts)], {stdio: ["pipe", "pipe", "pipe"]}) as never
		},
	})
	return {client, starts: () => starts}
}

describe("createPipeGeoDbClient", () => {
	it("keeps one worker across requests and parses the rows of the entity asked for", async () => {
		const {client, starts} = fakePipe(`
process.stdout.write("ready\\n")
lines((line) => {
	const [space, doc] = line.split("\\t")
	const entity = /id = "([0-9a-f]{32})"/.exec(doc)[1]
	frame("+", ["${PROP}", "Noam " + space.slice(0, 4), "stmt", entity, "1"].join("\\t") + "\\n" + ["${PROP}", "other", "stmt", "ffffffffffffffffffffffffffffffff", "1"].join("\\t") + "\\n")
})
`)
		for (let i = 0; i < 5; i++) {
			expect(await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).toEqual([
				{prop: PROP, raw: "Noam 2222"},
			])
		}
		expect(starts()).toBe(1)
		client.close()
	})

	it("answers many concurrent requests over the pool, each with its own rows", async () => {
		const {client, starts} = fakePipe(
			`
process.stdout.write("ready\\n")
lines((line) => {
	const doc = line.split("\\t")[1]
	const entity = /id = "([0-9a-f]{32})"/.exec(doc)[1]
	setTimeout(() => frame("+", ["${PROP}", entity, "stmt", entity, "1"].join("\\t") + "\\n"), 5)
})
`,
			{poolSize: 3},
		)
		const ids = Array.from({length: 40}, (_, i) => i.toString(16).padStart(32, "0"))
		const answers = await Promise.all(ids.map((entityId) => client.readCard({geoSpaceId: SPACE, entityId})))
		answers.forEach((rows, i) => {
			expect(rows).toEqual([{prop: PROP, raw: ids[i]}])
		})
		expect(starts()).toBe(3)
		client.close()
	})

	it("handles a frame split across writes and cells with escapes", async () => {
		const {client} = fakePipe(`
process.stdout.write("ready\\n")
lines((line) => {
	const entity = /id = "([0-9a-f]{32})"/.exec(line)[1]
	const text = ["${PROP}", "a\\\\tb\\\\nc", "stmt", entity, "1"].join("\\t") + "\\n"
	const whole = "+" + Buffer.byteLength(text) + "\\n" + text
	process.stdout.write(whole.slice(0, 7))
	setTimeout(() => process.stdout.write(whole.slice(7)), 20)
})
`)
		expect(await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).toEqual([{prop: PROP, raw: "a\tb\nc"}])
		client.close()
	})

	it("times out, kills the worker and starts a fresh one for the next request", async () => {
		const {client, starts} = fakePipe(
			`
process.stdout.write("ready\\n")
lines((line) => {
	const entity = /id = "([0-9a-f]{32})"/.exec(line)[1]
	if (process.argv[2] === "1") return // the first worker never answers
	frame("+", ["${PROP}", "ok", "stmt", entity, "1"].join("\\t") + "\\n")
})
`,
			{timeoutMs: 600, poolSize: 1},
		)
		await expect(client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).rejects.toMatchObject({reason: "timeout"})
		expect(await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).toEqual([{prop: PROP, raw: "ok"}])
		expect(starts()).toBe(2)
		client.close()
	})

	it("reports a refusal as client_failed and recovers on a fresh worker", async () => {
		const {client, starts} = fakePipe(
			`
process.stdout.write("ready\\n")
lines(() => {
	frame("-", "refused: invalid_argument bad_request")
	process.exit(1)
})
`,
			{poolSize: 1},
		)
		await expect(client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).rejects.toMatchObject({
			reason: "client_failed",
			message: expect.stringContaining("refused: invalid_argument"),
		})
		await expect(client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).rejects.toMatchObject({
			reason: "client_failed",
		})
		expect(starts()).toBe(2)
		client.close()
	})

	it("rejects the request a worker dies holding", async () => {
		const {client} = fakePipe(`
process.stdout.write("ready\\n")
lines(() => process.exit(3))
`)
		await expect(client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).rejects.toMatchObject({
			reason: "client_failed",
		})
		client.close()
	})

	it("backs off after a worker fails to start instead of starting one per request", async () => {
		const {client, starts} = fakePipe(`
console.error("--binding b: connect: ConnectionRefused")
process.exit(1)
`)
		await expect(client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).rejects.toMatchObject({
			reason: "client_failed",
			message: expect.stringContaining("ConnectionRefused"),
		})
		const started = starts()
		for (let i = 0; i < 20; i++) {
			await expect(client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).rejects.toMatchObject({
				reason: "client_failed",
			})
		}
		expect(starts()).toBe(started)
		client.close()
	})

	it("refuses ids that are not 32 hex digits before starting anything", async () => {
		const {client, starts} = fakePipe("process.exit(0)")
		await expect(client.readCard({geoSpaceId: SPACE, entityId: 'x" } }'})).rejects.toBeInstanceOf(GeoDbError)
		expect(starts()).toBe(0)
	})

	it("builds the entity card document", () => {
		expect(cardDocument(ENTITY)).toContain(`.id = "${ENTITY}"`)
	})
})

describe("createSpaceMap", () => {
	it("is empty without a path", async () => {
		expect(await createSpaceMap("")(SPACE)).toBeNull()
	})

	it("reads the shadow loader's state file", async () => {
		const dir = mkdtempSync(join(tmpdir(), "geodb-map-"))
		const path = join(dir, "spaces.json")
		writeFileSync(path, JSON.stringify({[SPACE]: {geo: ENTITY, nonce: "n"}}))
		const lookup = createSpaceMap(path)
		expect(await lookup(SPACE)).toBe(ENTITY)
		expect(await lookup(ENTITY)).toBeNull()
	})

	it("fails loudly when the file is unreadable", async () => {
		await expect(createSpaceMap("/nonexistent/spaces.json")(SPACE)).rejects.toMatchObject({reason: "client_failed"})
	})
})
