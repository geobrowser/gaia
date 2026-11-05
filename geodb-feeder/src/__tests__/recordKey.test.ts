import {execFileSync} from "node:child_process"
import {createHash} from "node:crypto"
import {existsSync, readFileSync} from "node:fs"
import {dirname, join} from "node:path"
import {fileURLToPath} from "node:url"
import {describe, expect, it} from "vitest"
import {
	dashed,
	intPart,
	type RecordKeyPart,
	recordEntityId,
	recordKeyId,
	recordKeyPreimage,
	recordStatementId,
	textPart,
} from "../recordKey.js"

/**
 * The golden vectors are vendored from geo-db (see `_vendored` in the file). Two guards against drift:
 * - VECTORS_DIGEST pins the vendored content, so a hand edit here fails;
 * - with GEODB_REPO set to a geo-db checkout, the file is compared with geo-db's `origin/final` (falling back to
 *   HEAD), so an upstream change to the frozen preimage fails here until it is re-vendored.
 */
const VENDORED_SHA = "9d97069a7899bed380b5ca01607dcf2cd331f81f"
const VECTORS_DIGEST = "3f9d463dc89dc8afac14a3b9bee65dba252f25014bc5093dfe78f3e9d67d0f9c"
const UPSTREAM_PATH = "tests/vectors/record_key/record-key.json"

type VectorPart = {id: string} | {bytes: string} | {int: string}
type Vector = {name: string; kind: string; role: "entity" | "statement"; parts: VectorPart[]; preimage: string; id: string}
type VectorFile = {_vendored?: string; contract: string; context: string; cases: Vector[]}

const here = dirname(fileURLToPath(import.meta.url))
const raw = readFileSync(join(here, "vectors/record-key.json"), "utf8")
const file = JSON.parse(raw) as VectorFile
const {_vendored, ...upstream} = file

const partOf = (p: VectorPart): RecordKeyPart =>
	"id" in p ? {id: p.id} : "bytes" in p ? {bytes: Uint8Array.from(Buffer.from(p.bytes, "hex"))} : {int: BigInt(p.int)}

describe("record key vectors (vendored)", () => {
	it("names the geo-db sha it came from", () => {
		expect(_vendored).toContain(VENDORED_SHA)
		expect(_vendored).toContain(UPSTREAM_PATH)
	})

	it("is the vendored content, unedited", () => {
		expect(createHash("sha256").update(JSON.stringify(upstream)).digest("hex")).toBe(VECTORS_DIGEST)
		expect(file.contract).toBe("geodb-record-key-vectors/v1")
		expect(file.context).toBe("geodb.v1.record.key")
		expect(file.cases).toHaveLength(13)
	})

	const repo = process.env.GEODB_REPO
	it.skipIf(!repo || !existsSync(join(repo ?? "", ".git")))("matches geo-db's current file (GEODB_REPO)", () => {
		const show = (ref: string) => execFileSync("git", ["-C", repo as string, "show", `${ref}:${UPSTREAM_PATH}`], {encoding: "utf8"})
		let text: string
		try {
			text = show("origin/final")
		} catch {
			text = show("HEAD")
		}
		expect(JSON.parse(text)).toEqual(upstream)
	})
})

describe("recordKeyId", () => {
	for (const v of file.cases) {
		it(`derives ${v.name} byte for byte`, () => {
			const parts = v.parts.map(partOf)
			expect(Buffer.from(recordKeyPreimage(v.kind, v.role, parts)).toString("hex")).toBe(v.preimage)
			const id = recordKeyId(v.kind, v.role, parts)
			expect(dashed(id)).toBe(v.id)
			expect(id).toMatch(/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/)
			const viaRole = v.role === "entity" ? recordEntityId(v.kind, parts) : recordStatementId(v.kind, parts)
			expect(viaRole).toBe(id)
		})
	}

	it("accepts compact and dashed ids alike", () => {
		const v = file.cases[0] as Vector
		const compact = v.parts.map((p) => ({id: (p as {id: string}).id.replaceAll("-", "")}))
		expect(dashed(recordKeyId(v.kind.replaceAll("-", ""), v.role, compact))).toBe(v.id)
	})

	it("text and int helpers encode as the vectors do", () => {
		const v = file.cases.find((c) => c.name === "text-utf8") as Vector
		expect(dashed(recordKeyId(v.kind, v.role, [textPart("Gödel")]))).toBe(v.id)
		const m = file.cases.find((c) => c.name === "column-int") as Vector
		const [rec, prop] = m.parts as [{id: string}, {id: string}]
		expect(dashed(recordStatementId(m.kind, [{id: rec.id}, {id: prop.id}, intPart(1)]))).toBe(m.id)
	})

	it("refuses what the engine refuses", () => {
		const kind = file.cases[0]?.kind as string
		expect(() => recordKeyId(kind, "entity", Array.from({length: 17}, () => intPart(0)))).toThrow(/at most 16/)
		expect(() => recordKeyId(kind, "entity", [intPart(-1)])).toThrow(/u64/)
		expect(() => recordKeyId(kind, "entity", [intPart(1n << 64n)])).toThrow(/u64/)
		expect(() => recordKeyId("not-an-id", "entity", [])).toThrow(/16-byte id/)
	})
})
