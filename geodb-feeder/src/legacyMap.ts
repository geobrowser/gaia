/**
 * The legacy id map (geo-db ISS-629-174, GAP-10; plan 10.12 section 4). gaia ids that are not UUID v4 are stored
 * under a v4 id the feeder derives (`conformingId`, a sha256 remap). geo-db cannot invert that, so each such entity
 * also gets one map statement
 *
 *   (keyed, sys.legacy.Id, Bytes(the original id's 16 raw bytes))
 *
 * whose id is `sys_ids.legacyStatementId(keyed, original)`: the record key of kind `sys.legacy.Entity` over
 * `[id keyed, id sys.legacy.Id, bytes original16]`. Its derivation is the one recordKey.ts already ports, so a
 * replay, a backfill and the live consumer state the same statement. The value is raw bytes, never hex text: a
 * value-range read needs an exact key, which only a short value has (geo-db `access/reads.zig` `boundKey`).
 *
 * The ids are the atoms' digests, pinned by `__tests__/vectors/legacy-map.json` against geo-db's own derivation.
 * `by_legacy` reads the statement and never recomputes the keyed id, so it answers for `conformingId`-keyed
 * entities as for `legacyEntityId`-keyed ones.
 */
import {bytesPart, idPart, recordStatementId} from "./recordKey.js"
import {isUuidV4} from "./ids.js"

/** `sys.legacy.Entity`, the record kind of the map statement's key (BLAKE3("geodb.v1.sys", "sys.legacy.Entity")). */
export const LEGACY_ENTITY_KIND = "76f0f36ed115499b820496a7b807b867"
/** `sys.legacy.Id`, the map property. */
export const LEGACY_ID_PROPERTY = "735016d93bbc4127b128a98e725c0762"

const HEX32 = /^[0-9a-f]{32}$/

/** An original id's compact lowercase form, or null when it is not 16 bytes of hex. */
export function compactOriginal(original: string): string | null {
	const compact = String(original).replaceAll("-", "").toLowerCase()
	return HEX32.test(compact) ? compact : null
}

/** `sys_ids.legacyStatementId(keyed, original)` as 32 lowercase hex digits. */
export function legacyStatementId(keyed: string, original: string): string {
	const compact = compactOriginal(original)
	if (compact === null) throw new Error(`legacy map: not a 16-byte id: ${original}`)
	return recordStatementId(LEGACY_ENTITY_KIND, [
		idPart(keyed),
		idPart(LEGACY_ID_PROPERTY),
		bytesPart(Uint8Array.from(Buffer.from(compact, "hex"))),
	])
}

/**
 * The `add` intent of the map statement for `keyed` (the feeder's id for `original`), or a reason it writes none:
 * `v4` (a v4 original is its own id: nothing to map) or `malformed` (not 16 bytes of hex). The Bytes value is
 * `.{ .bytes = "<32 hex>" }`; the geodb-client edit grammar must carry that form before the flag goes on.
 */
export function legacyMapIntent(keyed: string, original: string): {intent: string} | {skip: "v4" | "malformed"} {
	const compact = compactOriginal(original)
	if (compact === null) return {skip: "malformed"}
	if (isUuidV4(compact)) return {skip: "v4"}
	const sid = legacyStatementId(keyed, compact)
	return {
		intent: `.{ .add = .{ .id = "${sid}", .entity = "${keyed}", .property = "${LEGACY_ID_PROPERTY}", .value = .{ .bytes = "${compact}" } } }`,
	}
}
