import {spawn as nodeSpawn} from "node:child_process"
import {mkdtempSync, writeFileSync} from "node:fs"
import {tmpdir} from "node:os"
import {join} from "node:path"
import {describe, expect, it} from "vitest"
import {GeoDbError} from "../client"
import {
	countsLine,
	countsRefusal,
	createDerivedReader,
	FormGatedError,
	parseDerivedCounts,
	parseDerivedGet,
	parseDerivedScan,
	parseDerivedScores,
	parseNegotiated,
	refusalNeedsBackoff,
	refusalToken,
	type ScoreCard,
	type ScoreConfig,
	scoresLines,
} from "../derived"

const GROUP = "0".repeat(32)
const KEY = "ab".repeat(16)

/** A fake `geodb-client derived pipe`: a node script printing `ready`, then answering each line with a frame. */
function fake(body: string, timeoutMs = 2000, now?: () => number) {
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
			now,
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

describe("parseDerivedScan", () => {
	it("reads rows and the more marker", () => {
		expect(parseDerivedScan("2.5\t" + KEY + "\t\n1\t" + KEY + "\t00\nmore\n")).toEqual({
			rows: [
				{score: 2.5, keyHex: KEY, valueHex: ""},
				{score: 1, keyHex: KEY, valueHex: "00"},
			],
			more: true,
		})
		expect(parseDerivedScan("")).toEqual({rows: [], more: false})
	})
	it("rejects a malformed row", () => {
		expect(() => parseDerivedScan("nope\n")).toThrow(GeoDbError)
		expect(() => parseDerivedScan("x\t" + KEY + "\t\n")).toThrow(GeoDbError)
	})
})

describe("scan", () => {
	it("sends one scan line and parses the page", async () => {
		const {reader} = fake(
			`frame("+", line.startsWith("scan\\t0\\t0x${GROUP}\\t3\\t1") ? "4\\t${KEY}\\t\\nmore\\n" : "bad")`,
		)
		expect(await reader.scan(0, GROUP, 3)).toEqual({rows: [{score: 4, keyHex: KEY, valueHex: ""}], more: true})
		await reader.close()
	})
	it("refuses bad arguments without a read", async () => {
		const {reader, starts} = fake(`frame("+", "")`)
		await expect(reader.scan(0, "zz", 3)).rejects.toThrow(GeoDbError)
		await expect(reader.scan(0, GROUP, 0)).rejects.toThrow(GeoDbError)
		await expect(reader.scan(0, GROUP, 513)).rejects.toThrow(GeoDbError)
		expect(starts()).toBe(0)
		await reader.close()
	})
})

const CONFIG: ScoreConfig = {
	tauSeconds: 45000,
	wilsonZ: 1.96,
	priorPositive: 1,
	priorNegative: 1,
	qualityFloor: 0.01,
	intrinsicCap: 0.5,
	intrinsicPropertyTarget: 10,
	intrinsicRelationTarget: 5,
	participationWeight: 0,
	participationCap: 10,
	commentWeight: 0,
	commentCap: 30,
}
const CARD: ScoreCard = {
	entityHex: KEY,
	propertyCount: 4,
	relationCount: 2,
	createdEpoch: 1_760_000_000,
	typeWeightMax: 1.4,
}
const NONE: ScoreCard = {
	entityHex: "cd".repeat(16),
	propertyCount: 0,
	relationCount: 0,
	createdEpoch: null,
	typeWeightMax: null,
}
const ROW = (engaged: number, first: number, ranking: string, topic = "-") =>
	`${engaged}\t0\t${first}\t0\t0\t0\t0\t0.25\t0.5\t0\t0\t1.4\t0\t0\t0\t${ranking}\t${topic}`

describe("scores", () => {
	it("writes the head, the config and one card a line", () => {
		expect(scoresLines(CONFIG, [CARD, NONE]).split("\n")).toEqual([
			"scoresn\t2",
			"45000\t1.96\t1\t1\t0.01\t0.5\t10\t5\t0\t10\t0\t30",
			`${KEY}\t4\t2\t1760000000\t1.4`,
			`${"cd".repeat(16)}\t0\t0\t-\t-`,
		])
	})
	it("refuses a malformed card, an empty or oversized batch and a non-finite config", () => {
		expect(() => scoresLines(CONFIG, [])).toThrow(GeoDbError)
		expect(() => scoresLines(CONFIG, Array(65).fill(CARD))).toThrow(GeoDbError)
		expect(() => scoresLines(CONFIG, [{...CARD, entityHex: "zz"}])).toThrow(GeoDbError)
		expect(() => scoresLines(CONFIG, [{...CARD, propertyCount: -1}])).toThrow(GeoDbError)
		expect(() => scoresLines({...CONFIG, tauSeconds: Number.NaN}, [CARD])).toThrow(GeoDbError)
	})
	it("parses a row a card, a negative ranking and a topic, and refuses a malformed or short answer", () => {
		const topic = "ef".repeat(16)
		const rows = parseDerivedScores(`${ROW(1, 6, "-3.5", topic)}\n${ROW(0, 0, "2.25")}\n`, 2)
		expect(rows[0]).toMatchObject({engaged: true, counts: [6, 0, 0, 0, 0], ranking: -3.5, topicHex: topic})
		expect(rows[1]).toMatchObject({
			engaged: false,
			quality: 0.25,
			intrinsic: 0.5,
			typeWeight: 1.4,
			ranking: 2.25,
			topicHex: null,
		})
		expect(() => parseDerivedScores(`${ROW(0, 0, "1")}\n`, 2)).toThrow(GeoDbError)
		expect(() => parseDerivedScores(`${ROW(2, 0, "1")}\n`, 1)).toThrow(GeoDbError)
		expect(() => parseDerivedScores("0\t0\n", 1)).toThrow(GeoDbError)
		expect(() => parseDerivedScores(`${ROW(0, 0, "NaN")}\n`, 1)).toThrow(GeoDbError)
	})
	it("sends the lines of one request and reads one row a card", async () => {
		const {reader} = fake(
			String.raw`if (line === "revision") frame("-", "want")
else if (line.startsWith("scoresn\t")) { globalThis.left = Number(line.split("\t")[1]) + 1; globalThis.n = 1 }
else if (globalThis.left) { globalThis.n++; if (--globalThis.left === 0) frame("+", ["a","b"].map((e, i) => ["0","0",globalThis.n,"0","0","0","0","0.25","0.5","0","0","1.4","0","0","0",i + 2,"-"].join("\t")).join("\n") + "\n") }`,
		)
		const rows = await reader.scores(CONFIG, [CARD, NONE])
		// Four lines were sent: the head, the config and two cards.
		expect(rows.map((r) => r.counts[0])).toEqual([4, 4])
		expect(rows.map((r) => r.ranking)).toEqual([2, 3])
		await reader.close()
	})
	it("refuses a bad request without starting a worker", async () => {
		const {reader, starts} = fake(`frame("+", "")`)
		await expect(reader.scores(CONFIG, [])).rejects.toThrow(GeoDbError)
		expect(starts()).toBe(0)
		await reader.close()
	})
})

describe("counts", () => {
	const T1 = "11".repeat(16)
	const T2 = "22".repeat(16)
	const B1 = "33".repeat(16)

	it("writes the line with the group's 0x and a dash for no blocked", () => {
		expect(countsLine(1, GROUP, [T1, T2], [B1])).toBe(`counts\t1\t0x${GROUP}\t${T1},${T2}\t${B1}`)
		expect(countsLine(1, GROUP, [T1], [])).toBe(`counts\t1\t0x${GROUP}\t${T1}\t-`)
	})

	it("refuses an empty, repeated, oversized or malformed request", () => {
		const bad = (...args: Parameters<typeof countsLine>) => expect(() => countsLine(...args)).toThrow(GeoDbError)
		bad(1, GROUP, [], [])
		bad(1, GROUP, [T1, T1], [])
		bad(
			1,
			GROUP,
			Array.from({length: 65}, (_, i) => i.toString(16).padStart(32, "0")),
			[],
		)
		bad(
			1,
			GROUP,
			[T1],
			Array.from({length: 257}, (_, i) => i.toString(16).padStart(32, "0")),
		)
		bad(1, "xyz", [T1], [])
		bad(-1, GROUP, [T1], [])
		bad(1, GROUP, ["AB".repeat(16)], [])
	})

	it("parses the tallies in the order asked, with the walked rows", () => {
		expect(parseDerivedCounts(`${T1}\t5\n${T2}\t0\nwalked: 9\n`, [T1, T2])).toEqual({walked: 9, counts: [5, 0]})
	})

	it("refuses a short, reordered or malformed answer", () => {
		expect(() => parseDerivedCounts(`${T1}\t5\nwalked: 9\n`, [T1, T2])).toThrow(GeoDbError)
		expect(() => parseDerivedCounts(`${T2}\t5\nwalked: 9\n`, [T1])).toThrow(GeoDbError)
		expect(() => parseDerivedCounts(`${T1}\tx\nwalked: 9\n`, [T1])).toThrow(GeoDbError)
		expect(() => parseDerivedCounts(`${T1}\t5\n`, [T1])).toThrow(GeoDbError)
	})

	it("sends one counts line and reads the answer", async () => {
		const {reader} = fake(
			String.raw`if (line === "revision") return frame("-", "want"); const f = line.split("\t"); frame("+", f[3].split(",").map((t, i) => t + "\t" + (i + 1)).join("\n") + "\nwalked: 7\n")`,
		)
		expect(await reader.typeCounts(1, GROUP, [T1, T2], [])).toEqual({walked: 7, counts: [1, 2]})
		await expect(reader.typeCounts(1, GROUP, [], [])).rejects.toThrow(GeoDbError)
	})

	it("rejects a refusal, then recovers", async () => {
		const {reader} = fake(
			String.raw`if (line.startsWith("counts")) frame("-", "refused: InvalidWirePayload"); else frame("+", "")`,
		)
		await expect(reader.typeCounts(1, GROUP, [T1], [])).rejects.toThrow(GeoDbError)
	})
})

describe("a worker the serve has closed while idle", () => {
	const ANSWER = 'frame("+", "none")'
	const KEYHEX = "ab".repeat(16)

	it("is replaced before the read, not after it fails", async () => {
		let t = 1_000_000
		const {reader, starts} = fake(ANSWER, 2000, () => t)
		await reader.get(1, KEYHEX)
		expect(starts()).toBe(1)
		t += 30_000
		await reader.get(1, KEYHEX)
		expect(starts()).toBe(1)
		t += 46_000
		await reader.get(1, KEYHEX)
		expect(starts()).toBe(2)
		reader.close()
	})

	it("is replaced once it nears the end of its grant, even when never idle long", async () => {
		let t = 1_000_000
		const {reader, starts} = fake(ANSWER, 2000, () => t)
		for (let i = 0; i < 9; i++) {
			await reader.get(1, KEYHEX)
			t += 30_000
		}
		expect(starts()).toBe(2)
		reader.close()
	})

	it("answers the first read after a long idle even when the old worker already exited", async () => {
		let t = 1_000_000
		const {reader, starts} = fake(ANSWER, 2000, () => t)
		await reader.get(1, KEYHEX)
		t += 300_000
		expect(await reader.get(1, KEYHEX)).toBeNull()
		expect(starts()).toBe(2)
		reader.close()
	})
})

describe("the revision handshake and refusals that keep the pipe", () => {
	const T1 = "a1".repeat(16)
	const T2 = "b2".repeat(16)
	const OTHER = "c3".repeat(16)
	const GRAPH = {spaces: ["d4".repeat(16)], nameProperty: "e5".repeat(16), tagged: null}
	/** A new tool: answers `revision` with `negotiated`, a counts line with tallies, anything else per `rest`. */
	const newTool = (negotiated: number, rest = `frame("+", "none")`) => String.raw`
		if (line === "revision") frame("+", "negotiated ${negotiated}\tbuild 15")
		else if (line.startsWith("counts")) frame("+", line.split("\t")[3].split(",").map((t, i) => t + "\t" + (i + 1)).join("\n") + "\nwalked: 3\n")
		else { ${rest} }`

	it("sends the line `revision` first, then the request", async () => {
		const {reader} = fake(`
			globalThis.n = (globalThis.n ?? 0) + 1
			if (line === "revision") frame("+", "negotiated 15\tbuild 15")
			else frame("+", "${GROUP}\t" + n + "\t00")`)
		// The probe takes the first line (n=1); the get is the second.
		expect((await reader.get(1, "aa"))?.score).toBe(2)
		expect((await reader.get(1, "aa"))?.score).toBe(3)
		reader.close()
	})

	it("gates a form the negotiated revision is below, without sending it", async () => {
		const {reader} = fake(newTool(13))
		// Revision 13: the plain count goes; several topics (14) and a graph block (15) do not.
		await expect(reader.typeCounts(1, GROUP, [T1], [])).resolves.toEqual({walked: 3, counts: [1]})
		await expect(reader.typeCounts(1, GROUP, [T1], [], {others: [OTHER]})).rejects.toBeInstanceOf(FormGatedError)
		await expect(reader.typeCounts(1, GROUP, [T1], [], {graph: GRAPH})).rejects.toBeInstanceOf(FormGatedError)
		const gated = await reader.typeCounts(1, GROUP, [T1], [], {graph: GRAPH}).catch((e) => e)
		expect(countsRefusal(gated)).toBe("revision")
		expect(refusalNeedsBackoff(gated)).toBe(false)
		reader.close()
	})

	it("sends each form once the revision reaches it (14 for topics, 15 for a graph, 13 for the count)", async () => {
		const at14 = fake(newTool(14))
		await expect(
			at14.reader.typeCounts(1, GROUP, [T1, T2], [], {others: [OTHER], matchAll: true}),
		).resolves.toMatchObject({
			counts: [1, 2],
		})
		await expect(at14.reader.typeCounts(1, GROUP, [T1], [], {graph: GRAPH})).rejects.toBeInstanceOf(FormGatedError)
		at14.reader.close()
		const at15 = fake(newTool(15))
		await expect(at15.reader.typeCounts(1, GROUP, [T1], [], {graph: GRAPH})).resolves.toMatchObject({counts: [1]})
		at15.reader.close()
		const at12 = fake(newTool(12))
		await expect(at12.reader.typeCounts(1, GROUP, [T1], [])).rejects.toBeInstanceOf(FormGatedError)
		at12.reader.close()
	})

	it("treats a `-` answer to revision as an old tool: keeps the child, gates nothing, sends the form", async () => {
		const {reader, starts} = fake(`
			if (line === "revision") frame("-", "want put|delete|clear|get|scan|counts<TAB>...")
			else if (line.startsWith("counts")) frame("+", "${T1}\t4\\nwalked: 9\\n")
			else frame("+", "none")`)
		await expect(reader.typeCounts(1, GROUP, [T1], [], {graph: GRAPH})).resolves.toEqual({walked: 9, counts: [4]})
		await expect(reader.get(1, "aa")).resolves.toBeNull()
		expect(starts()).toBe(1)
		reader.close()
	})

	it("treats an unparsable `+` answer to revision as an old tool too", async () => {
		const {reader} = fake(`
			if (line === "revision") frame("+", "nonsense")
			else if (line.startsWith("counts")) frame("+", "${T1}\t4\\nwalked: 9\\n")`)
		await expect(reader.typeCounts(1, GROUP, [T1], [], {graph: GRAPH})).resolves.toMatchObject({counts: [4]})
		reader.close()
	})

	it("keeps the child through a node refusal, rejecting only that request", async () => {
		const {reader, starts} = fake(`
			if (line === "revision") frame("+", "negotiated 15\tbuild 15")
			else if (line.includes("hex:ee")) frame("-", "code=ResourceExhausted refused: DefiniteRefusal ResourceExhausted")
			else frame("+", "none")`)
		const refused = await reader.get(1, "ee").catch((e) => e)
		expect(refused).toBeInstanceOf(GeoDbError)
		expect(countsRefusal(refused)).toBe("exhausted")
		expect(refusalToken(refused)).toBe("ResourceExhausted")
		await expect(reader.get(1, "aa")).resolves.toBeNull()
		expect(starts()).toBe(1)
		reader.close()
	})

	it("keeps the child through InvalidWirePayload and UnknownOpcode, and answers queued requests after the refusal", async () => {
		const {reader, starts} = fake(`
			if (line === "revision") frame("+", "negotiated 15\tbuild 15")
			else if (line.includes("hex:e1")) frame("-", "code=InvalidWirePayload refused: DefiniteRefusal InvalidWirePayload")
			else if (line.includes("hex:e2")) frame("-", "code=UnknownOpcode refused: DefiniteRefusal UnknownOpcode")
			else frame("+", "none")`)
		const out = await Promise.allSettled([reader.get(1, "e1"), reader.get(1, "e2"), reader.get(1, "aa")])
		expect(out.map((o) => o.status)).toEqual(["rejected", "rejected", "fulfilled"])
		expect(starts()).toBe(1)
		reader.close()
	})

	it("keeps the child through a MalformedLine", async () => {
		const {reader, starts} = fake(`
			if (line === "revision") frame("+", "negotiated 15\tbuild 15")
			else if (line.includes("hex:ee")) frame("-", "code=MalformedLine want put|delete|clear|get|scan|counts<TAB>...")
			else frame("+", "none")`)
		await expect(reader.get(1, "ee")).rejects.toMatchObject({reason: "client_failed"})
		await expect(reader.get(1, "aa")).resolves.toBeNull()
		expect(starts()).toBe(1)
		reader.close()
	})

	it("kills the child on a non-refusal token (a protocol violation) and starts a fresh one", async () => {
		const {reader, starts} = fake(`
			if (line === "revision") frame("+", "negotiated 15\tbuild 15")
			else if (line.includes("hex:ee")) { frame("-", "code=ProtocolViolation error: ProtocolViolation"); process.exit(1) }
			else frame("+", "none")`)
		await expect(reader.get(1, "ee")).rejects.toMatchObject({reason: "client_failed"})
		await expect(reader.get(1, "aa")).resolves.toBeNull()
		expect(starts()).toBe(2)
		reader.close()
	})

	it("kills the child on an untokened refusal (a pre-2eab tool, whose refusal ends it)", async () => {
		const {reader, starts} = fake(`
			if (line === "revision") frame("-", "want ...")
			else if (line.includes("hex:ee")) frame("-", "refused: DefiniteRefusal ResourceExhausted")
			else frame("+", "none")`)
		const refused = await reader.get(1, "ee").catch((e) => e)
		expect(countsRefusal(refused)).toBe("exhausted")
		expect(refusalNeedsBackoff(refused)).toBe(true)
		await expect(reader.get(1, "aa")).resolves.toBeNull()
		expect(starts()).toBe(2)
		reader.close()
	})

	it("asks revision again on the replacement worker", async () => {
		let t = 1_000_000
		const {reader, starts} = fake(newTool(13), 2000, () => t)
		await expect(reader.typeCounts(1, GROUP, [T1], [], {graph: GRAPH})).rejects.toBeInstanceOf(FormGatedError)
		t += 50_000
		// A replaced worker learns the revision afresh, so a serve that rolled forward is seen without a restart of the api.
		await expect(reader.typeCounts(1, GROUP, [T1], [], {graph: GRAPH})).rejects.toBeInstanceOf(FormGatedError)
		expect(starts()).toBe(2)
		reader.close()
	})
})

describe("parseNegotiated and refusalToken", () => {
	it("reads the revision of a negotiated answer and nothing else", () => {
		expect(parseNegotiated("negotiated 15\tbuild 17")).toBe(15)
		expect(parseNegotiated("negotiated 15\tbuild 17\n")).toBe(15)
		expect(parseNegotiated("want put|delete")).toBeNull()
		expect(parseNegotiated("negotiated x\tbuild 1")).toBeNull()
	})
	it("reads the token of a refused error and null without one", () => {
		const e = (m: string) => new GeoDbError("client_failed", `derived read refused: ${m}`)
		expect(refusalToken(e("code=ResourceExhausted refused: x"))).toBe("ResourceExhausted")
		expect(refusalToken(e("refused: x"))).toBeNull()
		expect(refusalToken(new Error("code=X "))).toBeNull()
	})
})
