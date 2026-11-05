/**
 * Natural-key record ids, the TypeScript port of geo-db's `format.hasher.recordKeyId` (hasher registry row
 * `geodb.v1.record.key`; geo-db docs/architecture/02 §2, docs/api/05 §6, plan 10.12 K2). A row gaia keeps under a
 * composite natural key (a membership by (space, role, member), a vote by (proposal, voter)) is written under ids
 * derived from that key, so a replay, a backfill and the live consumer all land on the same statement with no
 * lookup table and no journal.
 *
 *   id = v4(BLAKE3 derive_key("geodb.v1.record.key", kind[16] || role[1] || part_0 || ... || part_(n-1))[0..16])
 *
 * role is 0x00 for an entity id and 0x01 for a statement id. Each part is self-delimiting: `0x01 || id[16]`,
 * `0x02 || u32_le(len) || bytes`, or `0x03 || u64_le`, at most 16 parts. The preimage is frozen by the engine; the
 * golden vectors in `__tests__/vectors/record-key.json` (vendored from geo-db) hold this port to it byte for byte.
 *
 * Ids in and out are the feeder's compact form (32 lowercase hex digits); dashed input is accepted.
 */
import {blake3} from "@noble/hashes/blake3"

export const RECORD_KEY_CONTEXT = "geodb.v1.record.key"
/** `record_key_parts_max` in geo-db. */
export const RECORD_KEY_PARTS_MAX = 16

export type RecordKeyRole = "entity" | "statement"
const ROLE_BYTE: Record<RecordKeyRole, number> = {entity: 0x00, statement: 0x01}

/** `RecordKeyPartTag`. */
const TAG_ID = 0x01
const TAG_BYTES = 0x02
const TAG_INT = 0x03
const U64_MAX = (1n << 64n) - 1n

/** One key part: a 16-byte id, a byte string (a text key is its canonical UTF-8 bytes), or an unsigned 64-bit integer. */
export type RecordKeyPart = {id: string} | {bytes: Uint8Array} | {int: bigint}

export const idPart = (id: string): RecordKeyPart => ({id})
export const bytesPart = (bytes: Uint8Array): RecordKeyPart => ({bytes})
/** A text part: its UTF-8 bytes as given. The caller picks the canonical form (lowercase for an address). */
export const textPart = (text: string): RecordKeyPart => ({bytes: new TextEncoder().encode(text)})
export const intPart = (n: bigint | number): RecordKeyPart => ({int: BigInt(n)})

function idBytes(id: string): Uint8Array {
	const compact = id.replaceAll("-", "").toLowerCase()
	if (!/^[0-9a-f]{32}$/.test(compact)) throw new Error(`record key: not a 16-byte id: ${id}`)
	return Uint8Array.from(Buffer.from(compact, "hex"))
}

/** The preimage `kind || role || parts`, exported so a test can hold it to the vectors' `preimage` too. */
export function recordKeyPreimage(kind: string, role: RecordKeyRole, parts: readonly RecordKeyPart[]): Uint8Array {
	if (parts.length > RECORD_KEY_PARTS_MAX) throw new Error(`record key: ${parts.length} parts, at most ${RECORD_KEY_PARTS_MAX}`)
	const chunks: Uint8Array[] = [idBytes(kind), Uint8Array.of(ROLE_BYTE[role])]
	for (const p of parts) {
		if ("id" in p) chunks.push(Uint8Array.of(TAG_ID), idBytes(p.id))
		else if ("bytes" in p) {
			if (p.bytes.length > 0xffffffff) throw new Error("record key: a bytes part longer than u32")
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

/** `recordKeyId(kind, role, parts)`: the derived id, UUID v4 stamped, as 32 lowercase hex digits. */
export function recordKeyId(kind: string, role: RecordKeyRole, parts: readonly RecordKeyPart[]): string {
	const digest = blake3(recordKeyPreimage(kind, role, parts), {context: RECORD_KEY_CONTEXT})
	const id = digest.slice(0, 16)
	id[6] = ((id[6] ?? 0) & 0x0f) | 0x40
	id[8] = ((id[8] ?? 0) & 0x3f) | 0x80
	return Buffer.from(id).toString("hex")
}

/** A record row's entity id: `recordKeyId(kind, entity, keys)`. */
export const recordEntityId = (kind: string, parts: readonly RecordKeyPart[]): string => recordKeyId(kind, "entity", parts)

/** A keyed statement id: an edge `(subject, role, target)`, a key reference `(record, property, target)`, or a column `(record, property, value)`. */
export const recordStatementId = (kind: string, parts: readonly RecordKeyPart[]): string => recordKeyId(kind, "statement", parts)

/** Canonical dashed UUID text for a compact id (the vectors' form). */
export function dashed(id: string): string {
	const c = id.replaceAll("-", "").toLowerCase()
	return `${c.slice(0, 8)}-${c.slice(8, 12)}-${c.slice(12, 16)}-${c.slice(16, 20)}-${c.slice(20)}`
}
