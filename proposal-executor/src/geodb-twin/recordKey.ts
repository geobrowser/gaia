/**
 * Natural-key record ids for the governance records: this package's copy of the feeder's `recordKey.ts` (geo-db
 * `format.hasher.recordKeyId`, context `geodb.v1.record.key`), as the api keeps one too. The executor cannot import
 * from either, so `tests/geodb-twin.test.ts` pins this copy to a vector the feeder's golden file also holds.
 *
 *   id = v4(BLAKE3 derive_key("geodb.v1.record.key", kind[16] || role[1] || part_0 || ...)[0..16])
 */
import {createHash} from "node:crypto"
import {blake3} from "@noble/hashes/blake3"

/**
 * geo-db's entity id for a gaia entity (a copy of the feeder's `cid("entity", id)`): ids that are already v4 are kept,
 * any other is remapped by a fixed hash. The id a record's reference column holds.
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

export type RecordKeyPart = {id: string}
export const idPart = (id: string): RecordKeyPart => ({id})

function idBytes(id: string): Uint8Array {
	const compact = id.replaceAll("-", "").toLowerCase()
	if (!/^[0-9a-f]{32}$/.test(compact)) throw new Error(`record key: not a 16-byte id: ${id}`)
	return Uint8Array.from(Buffer.from(compact, "hex"))
}

/** A record row's entity id: 32 lowercase hex digits. */
export function recordEntityId(kind: string, parts: readonly RecordKeyPart[]): string {
	const chunks: Uint8Array[] = [idBytes(kind), Uint8Array.of(0x00)]
	for (const p of parts) chunks.push(Uint8Array.of(TAG_ID), idBytes(p.id))
	const preimage = new Uint8Array(chunks.reduce((s, c) => s + c.length, 0))
	let at = 0
	for (const c of chunks) {
		preimage.set(c, at)
		at += c.length
	}
	const id = blake3(preimage, {context: RECORD_KEY_CONTEXT}).slice(0, 16)
	id[6] = ((id[6] ?? 0) & 0x0f) | 0x40
	id[8] = ((id[8] ?? 0) & 0x3f) | 0x80
	return Buffer.from(id).toString("hex")
}
