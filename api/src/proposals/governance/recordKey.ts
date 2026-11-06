/**
 * Natural-key record ids for the governance records, the api's copy of the feeder's `recordKey.ts` (geo-db
 * `format.hasher.recordKeyId`, context `geodb.v1.record.key`). Only the parts the shadow needs: an id part and an
 * unsigned integer part. The feeder's golden vectors (geodb-feeder/src/__tests__/vectors/record-key.json) hold this
 * copy to the engine byte for byte (`__tests__/governanceRecordKey.test.ts`).
 *
 *   id = v4(BLAKE3 derive_key("geodb.v1.record.key", kind[16] || role[1] || part_0 || ...)[0..16])
 */
import {createHash} from "node:crypto"
import {blake3} from "@noble/hashes/blake3"

/**
 * geo-db's entity id for a gaia entity (a copy of the feeder's `cid("entity", id)`, geodb-feeder/src/ids.ts): ids that
 * are already v4 are kept, any other is remapped by a fixed hash. The id a record's reference column holds.
 */
export function entityRef(gaiaId: string): string {
	const compact = gaiaId.replaceAll("-", "").toLowerCase()
	if (/^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/.test(compact)) return compact
	const bytes = Buffer.from(
		createHash("sha256").update(`geo:legacy-entity:v1\0${compact}`).digest("hex").slice(0, 32),
		"hex",
	)
	bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40
	bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80
	return bytes.toString("hex")
}

export const RECORD_KEY_CONTEXT = "geodb.v1.record.key"
const TAG_ID = 0x01
const TAG_BYTES = 0x02
const TAG_INT = 0x03
const U64_MAX = (1n << 64n) - 1n

export type RecordKeyPart = {id: string} | {bytes: Uint8Array} | {int: bigint}
export const idPart = (id: string): RecordKeyPart => ({id})
export const intPart = (n: bigint | number): RecordKeyPart => ({int: BigInt(n)})

function idBytes(id: string): Uint8Array {
	const compact = id.replaceAll("-", "").toLowerCase()
	if (!/^[0-9a-f]{32}$/.test(compact)) throw new Error(`record key: not a 16-byte id: ${id}`)
	return Uint8Array.from(Buffer.from(compact, "hex"))
}

export function recordKeyPreimage(
	kind: string,
	role: "entity" | "statement",
	parts: readonly RecordKeyPart[],
): Uint8Array {
	if (parts.length > 16) throw new Error(`record key: ${parts.length} parts, at most 16`)
	const chunks: Uint8Array[] = [idBytes(kind), Uint8Array.of(role === "entity" ? 0x00 : 0x01)]
	for (const p of parts) {
		if ("id" in p) chunks.push(Uint8Array.of(TAG_ID), idBytes(p.id))
		else if ("bytes" in p) {
			const len = new Uint8Array(4)
			new DataView(len.buffer).setUint32(0, p.bytes.length, true)
			chunks.push(Uint8Array.of(TAG_BYTES), len, p.bytes)
		} else {
			if (p.int < 0n || p.int > U64_MAX) throw new Error(`record key: an int part outside u64: ${p.int}`)
			const n = new Uint8Array(8)
			new DataView(n.buffer).setBigUint64(0, p.int, true)
			chunks.push(Uint8Array.of(TAG_INT), n)
		}
	}
	const out = new Uint8Array(chunks.reduce((s, c) => s + c.length, 0))
	let at = 0
	for (const c of chunks) {
		out.set(c, at)
		at += c.length
	}
	return out
}

/** A record row's entity id: 32 lowercase hex digits. */
export function recordEntityId(kind: string, parts: readonly RecordKeyPart[]): string {
	const id = blake3(recordKeyPreimage(kind, "entity", parts), {context: RECORD_KEY_CONTEXT}).slice(0, 16)
	id[6] = ((id[6] ?? 0) & 0x0f) | 0x40
	id[8] = ((id[8] ?? 0) & 0x3f) | 0x80
	return Buffer.from(id).toString("hex")
}
