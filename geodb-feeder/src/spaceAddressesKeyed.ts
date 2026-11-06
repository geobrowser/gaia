/**
 * Space addresses (`spaces.address`) as natural-key records (geo-db plan 10.12 K1-K5; the `governance` family), behind
 * GEODB_SPACE_ADDRESSES, which takes effect only with GEODB_RECORD_KEYS (spaceAddressesKeyedEnabled). With either off
 * nothing here runs. A flag of its own: the backfill must run before the consumer first starts.
 *
 * Source: `<env>.space.creations`, which carries two kinds of message, both of which kg-indexer folds into the `spaces`
 * row:
 * - SPACE_ADDRESS_UPDATED (`event-type` header; HermesSpaceAddressUpdated, space_id = 1 and address = 2, a 20-byte
 *   address): `UPDATE spaces SET address = '0x' || lowercase hex`. Postgres ignores it for a Space it never saw created;
 *   here it is written anyway (the gate reports such keys, it does not fail on them).
 * - HermesCreateSpace (no header; space_id = 1, eoa_space = 3 {owner = 1} or default_dao_space = 4 {address = 1}):
 *   `INSERT ... ON CONFLICT DO NOTHING` of the EIP-55 CHECKSUMMED address (indexer_utils::checksum_address).
 *   A create that has neither payload is an error in kg-indexer and is dead-lettered here.
 * Every other event type is ignored. One record per Space, in the `governance` system Space:
 * - record entity: `recordEntityId(spaceAddress, [id(space)])`;
 * - key column (RECORD_COLUMNS.spaceAddress.space): `add` + `set_active(true)`, a reference to the Space's entity;
 * - mutable column `address`: a `put` of the address text under `recordStatementId(spaceAddress, [record, addressProperty,
 *   text(address)])`. A put closes the cell's other active statements, so a new address replaces the old one, and the
 *   value in the id makes a replay a no-op and A -> B -> A end on A.
 *
 * Known difference: ON CONFLICT DO NOTHING needs a read of the current row, which a keyed write cannot do. A create
 * for a Space that already has a record is therefore written as a put, and would overwrite an address that an
 * SPACE_ADDRESS_UPDATED set earlier. kg-indexer emits a create once per Space (a re-registration is not a normal path),
 * so this is documented rather than guarded; the gate (spaceAddressesGate.ts) shows it if it ever happens.
 * The casing also differs by path, as in Postgres: a create stores checksummed, an update lowercase.
 */
import {keccak_256} from "@noble/hashes/sha3"
import {parseFields} from "./hermes.js"
import {cid} from "./ids.js"
import {
	applyKeyedRecord,
	backfillKeyedRows,
	deadLetterDecode,
	type KeyedBackfillResult,
	type KeyedOutcome,
	type KeyedRecordDeps,
	keyColumn,
} from "./keyedRecord.js"
import {idPart, recordEntityId, recordStatementId, textPart} from "./recordKey.js"
import {RECORD_COLUMNS, RECORD_KINDS} from "./recordKinds.js"
import {putIntent, zigString} from "./translate.js"

const KIND = RECORD_KINDS.spaceAddress.id
const FAMILY = RECORD_KINDS.spaceAddress.family
const COL = RECORD_COLUMNS.spaceAddress

export const SPACE_ADDRESS_UPDATED = "SPACE_ADDRESS_UPDATED"

export type SpaceAddressEvent = {space: string; address: string}

/** indexer_utils::checksum_address: EIP-55 over the lowercase hex, `0x` prefixed. `hex` is 40 hex characters. */
export function checksumAddress(hex: string): string {
	const lower = hex.toLowerCase().replace(/^0x/, "")
	const hash = keccak_256(new TextEncoder().encode(lower))
	let out = "0x"
	for (let i = 0; i < 40; i++) {
		const nibble = i % 2 === 0 ? hash[i >> 1]! >> 4 : hash[i >> 1]! & 0x0f
		out += nibble >= 8 ? lower[i]!.toUpperCase() : lower[i]!
	}
	return out
}

const UUID16 = (v: bigint | Uint8Array | undefined, what: string): string => {
	if (!(v instanceof Uint8Array) || v.length !== 16) throw new Error(`space address: ${what} is not 16 bytes`)
	return Buffer.from(v).toString("hex")
}

const ADDRESS20 = (v: Uint8Array | undefined): Uint8Array => {
	if (!(v instanceof Uint8Array) || v.length !== 20) throw new Error("space address: address is not 20 bytes")
	return v
}

/** Decode HermesSpaceAddressUpdated. Throws where kg-indexer errors on the space id, or on an address that is not 20 bytes. */
export function parseAddressUpdated(b: Uint8Array): SpaceAddressEvent {
	const f = new Map<number, bigint | Uint8Array>()
	for (const [n, v] of parseFields(b)) f.set(n, v)
	return {space: UUID16(f.get(1), "space_id"), address: `0x${Buffer.from(ADDRESS20(f.get(2) as Uint8Array | undefined)).toString("hex")}`}
}

/** Decode HermesCreateSpace to the checksummed address kg-indexer inserts. Throws on a message without a payload. */
export function parseCreateSpaceAddress(b: Uint8Array): SpaceAddressEvent {
	let space: bigint | Uint8Array | undefined
	let payload: Uint8Array | undefined
	for (const [n, v] of parseFields(b)) {
		if (n === 1) space = v
		else if ((n === 3 || n === 4) && v instanceof Uint8Array) payload = v
	}
	const id = UUID16(space, "space_id")
	if (!payload) throw new Error("space address: create without a payload")
	let address: Uint8Array | undefined
	for (const [n, v] of parseFields(payload)) if (n === 1 && v instanceof Uint8Array) address = v
	return {space: id, address: checksumAddress(Buffer.from(ADDRESS20(address)).toString("hex"))}
}

/** The record entity of one Space's address. */
export const spaceAddressRecordId = (space: string): string => recordEntityId(KIND, [idPart(space)])

/** The intents for one event. Pure, and depends on the event alone. */
export function spaceAddressIntents(e: SpaceAddressEvent): string[] {
	const record = spaceAddressRecordId(e.space)
	const space = cid("entity", e.space)
	return [
		`.{ .create_entity = "${record}" }`,
		...keyColumn(recordStatementId(KIND, [idPart(record), idPart(COL.space), idPart(space)]), record, COL.space, `.{ .reference = "${space}" }`),
		putIntent({
			id: recordStatementId(KIND, [idPart(record), idPart(COL.address), textPart(e.address)]),
			entity: record,
			property: COL.address,
			value: `.{ .text = ${zigString(e.address)} }`,
		}),
	]
}

export type KeyedSpaceAddressesDeps = KeyedRecordDeps

export async function applySpaceAddress(
	deps: KeyedSpaceAddressesDeps,
	e: SpaceAddressEvent,
	meta: {partition: number; offset: string},
): Promise<KeyedOutcome> {
	return applyKeyedRecord(deps, FAMILY, spaceAddressIntents(e), {...meta, space: e.space})
}

const headerText = (h: unknown): string | undefined => {
	const v = Array.isArray(h) ? h[0] : h
	if (v === undefined || v === null) return undefined
	return Buffer.isBuffer(v) || v instanceof Uint8Array ? Buffer.from(v).toString("utf8") : String(v)
}

export async function handleSpaceAddressMessage(
	deps: KeyedSpaceAddressesDeps,
	partition: number,
	message: {offset: string; value: Uint8Array | null; headers?: Record<string, unknown>},
): Promise<KeyedOutcome> {
	const t = headerText(message.headers?.["event-type"])
	// HermesCreateSpace has no event-type header; SPACE_ADDRESS_UPDATED has. Any other header value is not ours.
	if ((t !== undefined && t !== SPACE_ADDRESS_UPDATED) || !message.value) return "ignored"
	let e: SpaceAddressEvent
	try {
		e = t === undefined ? parseCreateSpaceAddress(message.value) : parseAddressUpdated(message.value)
	} catch (err) {
		return deadLetterDecode(deps, "space.creations", partition, message.offset, err)
	}
	return applySpaceAddress(deps, e, {partition, offset: message.offset})
}

/** Whether the keyed address writer runs: GEODB_SPACE_ADDRESSES only takes effect with GEODB_RECORD_KEYS. */
export const spaceAddressesKeyedEnabled = (c: {recordKeys: boolean; spaceAddresses: {enabled: boolean}}): boolean => c.spaceAddresses.enabled && c.recordKeys

/** The consumer's offsets directory under DATA_DIR; the backfill gates on it (keyedBackfillGate). */
export const spaceAddressesOffsetsDir = (dataDir: string) => `${dataDir}/feeder/space-addresses-keyed`

/** A `spaces` row, as the backfill reads it (pgSource.readSpaceAddressRows). */
export type SpaceAddressRow = {space_id: string; address: string}

const UUID = /^[0-9a-f]{32}$/

/** A Postgres row as the event that set it; the address is kept exactly as stored. Throws on a row no event could have written. */
export function spaceAddressFromRow(r: SpaceAddressRow): SpaceAddressEvent {
	const space = String(r.space_id).replaceAll("-", "").toLowerCase()
	const address = String(r.address)
	if (!UUID.test(space) || !/^0x[0-9a-fA-F]{40}$/.test(address)) throw new Error(`spaces: address row ${JSON.stringify(r)}`)
	return {space, address}
}

/** Write a snapshot of `spaces.address`. Rerunnable: a held row is written again to the same ids. */
export const backfillSpaceAddresses = (
	deps: KeyedSpaceAddressesDeps,
	rows: AsyncIterable<SpaceAddressRow> | Iterable<SpaceAddressRow>,
	progress?: (done: number) => void,
): Promise<KeyedBackfillResult> =>
	backfillKeyedRows(
		deps,
		rows,
		{topic: "space.creations", fromRow: spaceAddressFromRow, apply: (d, e) => applySpaceAddress(d, e, {partition: -1, offset: "backfill"})},
		progress,
	)
