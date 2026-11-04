import {spawn as nodeSpawn} from "node:child_process"
import {mkdtempSync, readFileSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {cardDocument, createPipeGeoDbClient, createSpaceMap, GeoDbError} from "../client"
import type {GeoDbCardConfig} from "../config"
import {activateGeoDbMetrics, renderGeoDbCardMetrics} from "../metrics"

const ENTITY = "073ff16be7fe4591a984c325d629d92d"
const SPACE = "22222222222242228222222222222222"
const PROP = "98012398829d48b083738f2589675894"

/** A fake `geodb-client pipe`: a node script speaking the frame protocol, run through the client's spawn seam. */
function fakePipe(
	body: string,
	options: {timeoutMs?: number; poolSize?: number; keepWarmMs?: number; grantTtlMs?: number} = {},
) {
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
		...(options.keepWarmMs ? {keepWarmMs: options.keepWarmMs} : {}),
		...(options.grantTtlMs ? {grantTtlMs: options.grantTtlMs} : {}),
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

	it("keeps a worker warm with an empty read once it has idled, and never before a first read", async () => {
		const dir = mkdtempSync(join(tmpdir(), "geodb-warm-"))
		const log = join(dir, "lines.log")
		const {client, starts} = fakePipe(
			`
import {appendFileSync} from "node:fs"
process.stdout.write("ready\\n")
lines((line) => {
	appendFileSync(${JSON.stringify(log)}, line + "\\n")
	frame("+", "")
})
`,
			{keepWarmMs: 60, poolSize: 1},
		)
		await new Promise((r) => setTimeout(r, 200))
		expect(starts()).toBe(0)
		await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})
		await new Promise((r) => setTimeout(r, 500))
		const lines = readFileSync(log, "utf8").trim().split("\n")
		expect(lines.length).toBeGreaterThan(2)
		expect(lines[0]).toContain(ENTITY)
		for (const line of lines.slice(1)) {
			expect(line.startsWith(`${SPACE}\t`)).toBe(true)
			expect(line).toContain("0123456789ab4def8123456789abcdef")
		}
		expect(starts()).toBe(1)
		client.close()
		const after = readFileSync(log, "utf8").length
		await new Promise((r) => setTimeout(r, 200))
		expect(readFileSync(log, "utf8").length).toBe(after)
	})

	it("once it has read, starts every slot's worker ahead of need and warms each", async () => {
		const dir = mkdtempSync(join(tmpdir(), "geodb-ahead-"))
		const log = join(dir, "lines.log")
		const {client, starts} = fakePipe(
			`
import {appendFileSync} from "node:fs"
process.stdout.write("ready\\n")
lines((line) => {
	appendFileSync(${JSON.stringify(log)}, process.argv[2] + " " + line + "\\n")
	frame("+", "")
})
`,
			{keepWarmMs: 60, poolSize: 3},
		)
		await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})
		await new Promise((r) => setTimeout(r, 600))
		expect(starts()).toBe(3)
		const lines = readFileSync(log, "utf8").trim().split("\n")
		// Every worker, including the two that have answered no read, is sent a keep-warm read in the last Space.
		for (const worker of ["1", "2", "3"]) {
			expect(
				lines.some(
					(l) => l.startsWith(`${worker} ${SPACE}\t`) && l.includes("0123456789ab4def8123456789abcdef"),
				),
			).toBe(true)
		}
		// A burst as wide as the pool now finds every worker ready: no worker is started for it.
		await Promise.all([1, 2, 3].map(() => client.readCard({geoSpaceId: SPACE, entityId: ENTITY})))
		expect(starts()).toBe(3)
		client.close()
	})

	it("replaces an idle worker before its token expires, so no read finds the token dead", async () => {
		const {client, starts} = fakePipe(
			`
const born = Date.now()
process.stdout.write("ready\\n")
lines((line) => {
	if (Date.now() - born > 400) frame("-", "error: CapabilityTokenExpired")
	else frame("+", "")
})
`,
			{keepWarmMs: 250, poolSize: 1, grantTtlMs: 400},
		)
		activateGeoDbMetrics()
		const ends = (reason: string) =>
			Number(
				new RegExp(`gaia_api_geodb_worker_ends_total\\{reason="${reason}"\\} (\\d+)`).exec(
					renderGeoDbCardMetrics(),
				)?.[1],
			)
		const recycledBefore = ends("recycled")
		const deadBefore = ends("dead_session")
		await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})
		await new Promise((r) => setTimeout(r, 900))
		await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})
		// Retired at half its 400 ms lifetime, twice or more in 900 ms; the read never met an expired token.
		expect(starts()).toBeGreaterThanOrEqual(3)
		expect(ends("recycled") - recycledBefore).toBeGreaterThanOrEqual(2)
		expect(ends("dead_session") - deadBefore).toBe(0)
		client.close()
	})

	it("gives a slot whose worker died a fresh one before the next read needs it", async () => {
		const {client, starts} = fakePipe(
			`
process.stdout.write("ready\\n")
lines((line) => {
	if (process.argv[2] === "1") frame("-", "error: NotFound")
	else frame("+", "")
})
`,
			{keepWarmMs: 60, poolSize: 1},
		)
		await expect(client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).rejects.toMatchObject({
			reason: "client_failed",
		})
		await new Promise((r) => setTimeout(r, 200))
		expect(starts()).toBe(2)
		await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})
		expect(starts()).toBe(2)
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

	it.each([
		["TransportClosed", "error: TransportClosed\\n"],
		["CapabilityTokenExpired", "refused: DefiniteRefusal CapabilityTokenExpired"],
	])("replaces a worker that answers %s and retries the request once on a fresh one", async (_name, text) => {
		const {client, starts} = fakePipe(
			`
process.stdout.write("ready\\n")
lines((line) => {
	if (process.argv[2] === "1") {
		frame("-", "${text}")
		process.exit(1)
	}
	const entity = /id = "([0-9a-f]{32})"/.exec(line)[1]
	frame("+", ["${PROP}", "fresh", "stmt", entity, "1"].join("\t") + "\\n")
})
`,
			{poolSize: 1},
		)
		expect(await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).toEqual([{prop: PROP, raw: "fresh"}])
		expect(starts()).toBe(2)
		expect(await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).toEqual([{prop: PROP, raw: "fresh"}])
		expect(starts()).toBe(2)
		client.close()
	})

	it("retries a request whose started worker exits without answering", async () => {
		const {client, starts} = fakePipe(
			`
process.stdout.write("ready\\n")
lines((line) => {
	if (process.argv[2] === "1") process.exit(1)
	const entity = /id = "([0-9a-f]{32})"/.exec(line)[1]
	frame("+", ["${PROP}", "fresh", "stmt", entity, "1"].join("\t") + "\\n")
})
`,
			{poolSize: 1},
		)
		expect(await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).toEqual([{prop: PROP, raw: "fresh"}])
		expect(starts()).toBe(2)
		client.close()
	})

	it("does not retry a real refusal", async () => {
		const {client, starts} = fakePipe(
			`
process.stdout.write("ready\\n")
lines(() => {
	frame("-", "refused: DefiniteRefusal NotFoundOrRestricted")
	process.exit(1)
})
`,
			{poolSize: 1},
		)
		await expect(client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).rejects.toMatchObject({
			reason: "client_failed",
			message: expect.stringContaining("NotFoundOrRestricted"),
		})
		expect(starts()).toBe(1)
		client.close()
	})

	it("retries a request only once, and caps retries when every worker keeps dying", async () => {
		const {client, starts} = fakePipe(
			`
process.stdout.write("ready\\n")
lines(() => {
	frame("-", "error: TransportClosed")
	process.exit(1)
})
`,
			{poolSize: 1},
		)
		await expect(client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).rejects.toMatchObject({
			reason: "client_failed",
			message: expect.stringContaining("TransportClosed"),
		})
		expect(starts()).toBe(2) // the original worker and one retry, not a loop
		for (let i = 0; i < 29; i++) {
			await expect(client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).rejects.toMatchObject({
				reason: "client_failed",
			})
		}
		// 30 requests: one worker each, plus at most 8 retries in the window (uncapped would be 60).
		expect(starts()).toBeLessThanOrEqual(30 + 8)
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

	it("keeps the default card document byte-identical, and asks for names only on request", () => {
		const plain = `.{ .version = 1, .select = .{ .c = .{ .field = "entity", .args = .{ .id = "${ENTITY}" } } } }`
		expect(cardDocument(ENTITY)).toBe(plain)
		expect(cardDocument(ENTITY, {})).toBe(plain)
		expect(cardDocument(ENTITY, {names: false})).toBe(plain)
		expect(cardDocument(ENTITY, {names: true})).toBe(
			`.{ .version = 1, .select = .{ .c = .{ .field = "entity", .args = .{ .id = "${ENTITY}", .names = true } } } }`,
		)
	})

	it("sends `.names = true` and parses the names cells only when the read asks for names", async () => {
		const {client} = fakePipe(`
process.stdout.write("ready\\n")
lines((line) => {
	const [, doc] = line.split("\\t")
	const entity = /id = "([0-9a-f]{32})"/.exec(doc)[1]
	const names = doc.includes(".names = true")
	frame("+", ["${PROP}", "v", "stmt", entity, ...(names ? ["Prop", "-"] : []), "1"].join("\\t") + "\\n")
})
`)
		expect(await client.readCard({geoSpaceId: SPACE, entityId: ENTITY})).toEqual([{prop: PROP, raw: "v"}])
		expect(await client.readCard({geoSpaceId: SPACE, entityId: ENTITY, names: true})).toEqual([
			{prop: PROP, raw: "v", propName: "Prop", targetName: null},
		])
	})
})

describe("createPipeGeoDbClient, several spaces a line", () => {
	const OTHER = "33333333333343338333333333333333"

	it("sends `<space>,<space>\\t<doc>` and reads the `<space>:<statement>` cells into each row's space", async () => {
		const {client} = fakePipe(`
process.stdout.write("ready\\n")
lines((line) => {
	const [spaces, doc] = line.split("\\t")
	const entity = /id = "([0-9a-f]{32})"/.exec(doc)[1]
	frame("+", spaces.split(",").map((s) => ["${PROP}", "v" + s.slice(0, 2), s + ":stmt", entity, "1"].join("\\t")).join("\\n") + "\\n")
})
`)
		const rows = await client.readCardSpaces?.({geoSpaceIds: [SPACE, OTHER], entityId: ENTITY})
		expect(rows).toEqual([
			{prop: PROP, raw: "v22", space: SPACE},
			{prop: PROP, raw: "v33", space: OTHER},
		])
		client.close()
	})

	it("refuses a repeated, malformed or empty list before it reaches the worker", async () => {
		const {client, starts} = fakePipe(`process.stdout.write("ready\\n")`)
		for (const geoSpaceIds of [[], [SPACE, SPACE], [SPACE, "nope"]])
			await expect(client.readCardSpaces?.({geoSpaceIds, entityId: ENTITY})).rejects.toMatchObject({
				reason: "bad_request",
			})
		expect(starts()).toBe(0)
	})
})

describe("createSpaceMap", () => {
	it("is empty without a path", async () => {
		expect(await createSpaceMap("")(SPACE)).toBeNull()
		expect(await createSpaceMap("").all()).toEqual([])
	})

	it("lists every distinct geo-db Space it holds", async () => {
		const dir = mkdtempSync(join(tmpdir(), "geodb-map-"))
		const path = join(dir, "spaces.json")
		writeFileSync(path, JSON.stringify({[SPACE]: {geo: ENTITY}, [PROP]: {geo: ENTITY}, [ENTITY]: {geo: PROP}}))
		expect((await createSpaceMap(path).all()).sort()).toEqual([PROP, ENTITY].sort())
	})

	it("reads the shadow loader's state file", async () => {
		const dir = mkdtempSync(join(tmpdir(), "geodb-map-"))
		const path = join(dir, "spaces.json")
		writeFileSync(path, JSON.stringify({[SPACE]: {geo: ENTITY}}))
		const lookup = createSpaceMap(path)
		expect(await lookup(SPACE)).toBe(ENTITY)
		expect(await lookup(ENTITY)).toBeNull()
	})

	it("picks up a Space the feeder adds, and keeps the old map if the file turns bad", async () => {
		const dir = mkdtempSync(join(tmpdir(), "geodb-map-"))
		const path = join(dir, "spaces.json")
		writeFileSync(path, JSON.stringify({[SPACE]: {geo: ENTITY}}))
		let clock = 0
		const lookup = createSpaceMap(path, {refreshMs: 1000, now: () => clock})
		expect(await lookup(SPACE)).toBe(ENTITY)
		expect(await lookup(ENTITY)).toBeNull()
		writeFileSync(path, JSON.stringify({[SPACE]: {geo: ENTITY}, [ENTITY]: {geo: SPACE}}))
		expect(await lookup(ENTITY)).toBeNull() // inside the refresh window
		clock = 1500
		expect(await lookup(ENTITY)).toBe(SPACE)
		writeFileSync(path, "{not json at all, longer than before")
		clock = 3000
		expect(await lookup(SPACE)).toBe(ENTITY)
	})

	it("treats a Space marked `loaded: false` as not loaded, and one without the field as loaded", async () => {
		const dir = mkdtempSync(join(tmpdir(), "geodb-map-"))
		const path = join(dir, "spaces.json")
		writeFileSync(
			path,
			JSON.stringify({
				[SPACE]: {geo: ENTITY, loaded: false},
				[ENTITY]: {geo: SPACE},
				[PROP]: {geo: PROP, loaded: true},
			}),
		)
		const lookup = createSpaceMap(path)
		expect(await lookup(SPACE)).toBeNull()
		expect(await lookup(ENTITY)).toBe(SPACE)
		expect(await lookup(PROP)).toBe(PROP)
		expect((await lookup.all()).sort()).toEqual([PROP, SPACE].sort())
	})

	it("refuses a file with a malformed entry", async () => {
		const dir = mkdtempSync(join(tmpdir(), "geodb-map-"))
		const path = join(dir, "spaces.json")
		writeFileSync(path, JSON.stringify({[SPACE]: {geo: ENTITY}, nope: {geo: "x"}}))
		await expect(createSpaceMap(path)(SPACE)).rejects.toMatchObject({reason: "client_failed"})
	})

	it("fails loudly when the file is unreadable", async () => {
		await expect(createSpaceMap("/nonexistent/spaces.json")(SPACE)).rejects.toMatchObject({reason: "client_failed"})
	})
})
