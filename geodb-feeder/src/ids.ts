import {createHash} from "node:crypto"

/** GRC-20 ids decode as 16-byte arrays; hash and format them as 32 hex digits, never as String(bytes). */
export function hexId(x: string | Uint8Array): string {
	return typeof x === "string" ? x : Buffer.from(x).toString("hex")
}

export function isUuidV4(value: string): boolean {
	return /^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$/i.test(value)
}

/**
 * geo-db wants RFC 4122 v4 ids. Ids that already are v4 are kept; any other id is remapped
 * deterministically, so a replay always produces the same geo-db id.
 */
export function conformingId(kind: string, legacyId: string): string {
	const compact = String(legacyId).replaceAll("-", "").toLowerCase()
	if (isUuidV4(compact)) return compact
	const bytes = Buffer.from(
		createHash("sha256").update(`geo:legacy-${kind}:v1\0${compact}`).digest("hex").slice(0, 32),
		"hex",
	)
	bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40
	bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80
	return bytes.toString("hex")
}

export const cid = (kind: string, id: string | Uint8Array): string => conformingId(kind, hexId(id))

/** The founding nonce of a gaia Space. Deterministic, so a re-founded Space gets the same geo-db id. */
export function nonceOf(gaiaSpace: string): string {
	const h = createHash("sha256").update(`shadow-nonce:${gaiaSpace}`).digest().subarray(0, 16)
	h[6] = ((h[6] ?? 0) & 0x0f) | 0x40
	h[8] = ((h[8] ?? 0) & 0x3f) | 0x80
	return h.toString("hex")
}
