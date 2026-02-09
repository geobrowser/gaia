/**
 * Base58 encoding/decoding for UUIDs.
 *
 * Ported from indexer_utils/src/id.rs to produce identical output.
 * Uses the Bitcoin Base58 alphabet (no 0, O, I, l).
 *
 * UUIDs are 128-bit values. Base58-encoded, they produce ~22 character strings
 * (variable length — leading zero bytes produce shorter output, no padding).
 *
 * NOTE: The zero UUID (all zeros) encodes to an empty string. This matches
 * the Rust implementation. The roundtrip encode→decode is broken for zero
 * because decodeBase58("") throws. Zero UUIDs should not appear in production
 * data; if they do, callers must handle the empty string.
 */

const BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"

/** Max encoded length for a 128-bit UUID in Base58. */
const MAX_BASE58_LENGTH = 22

const HEX32_PATTERN = /^[0-9a-f]{32}$/

// Lookup table: char code → alphabet index (255 = invalid)
const BASE58_DECODE_MAP = new Uint8Array(128).fill(255)
for (let i = 0; i < BASE58_ALPHABET.length; i++) {
	BASE58_DECODE_MAP[BASE58_ALPHABET.charCodeAt(i)] = i
}

/**
 * Encode a dashless hex string to Base58.
 *
 * Matches the Rust `encode_uuid_to_base58` implementation exactly:
 * no zero-padding, empty string for zero value.
 *
 * @param dashlessHex - 32-char lowercase hex string (no dashes)
 * @throws if the input is not exactly 32 lowercase hex characters
 */
/** Bounded FIFO cache for encodeBase58 — avoids repeated BigInt work for recurring UUIDs. */
const ENCODE_CACHE_MAX = 2048
const encodeCache = new Map<string, string>()

export function encodeBase58(dashlessHex: string): string {
	if (!HEX32_PATTERN.test(dashlessHex)) {
		throw new Error(`encodeBase58: expected 32-char lowercase hex, got ${dashlessHex.length} chars`)
	}

	const cached = encodeCache.get(dashlessHex)
	if (cached !== undefined) return cached

	let remainder = BigInt(`0x${dashlessHex}`)
	if (remainder === 0n) return ""

	const chars: string[] = []
	let iterations = 0
	while (remainder > 0n) {
		if (++iterations > MAX_BASE58_LENGTH) {
			throw new Error(`encodeBase58: loop exceeded ${MAX_BASE58_LENGTH} iterations — input may be invalid`)
		}
		const mod = Number(remainder % 58n)
		chars.push(BASE58_ALPHABET[mod])
		remainder /= 58n
	}

	chars.reverse()
	const result = chars.join("")

	// FIFO eviction: delete oldest entry when cache is full
	if (encodeCache.size >= ENCODE_CACHE_MAX) {
		const firstKey = encodeCache.keys().next().value
		if (firstKey !== undefined) encodeCache.delete(firstKey)
	}
	encodeCache.set(dashlessHex, result)

	return result
}

/**
 * Decode a Base58 string to a dashless hex string (32 chars, zero-padded).
 *
 * Matches the Rust `decode_base58_to_uuid` implementation exactly.
 *
 * @throws if the input contains invalid Base58 characters, is empty, or overflows 128 bits
 */
export function decodeBase58(encoded: string): string {
	if (encoded.length === 0) {
		throw new Error("Invalid Base58: empty string")
	}

	if (encoded.length > MAX_BASE58_LENGTH) {
		throw new Error(`Invalid Base58: length ${encoded.length} exceeds maximum ${MAX_BASE58_LENGTH}`)
	}

	let decoded = 0n
	for (let i = 0; i < encoded.length; i++) {
		const charCode = encoded.charCodeAt(i)
		if (charCode >= 128) {
			throw new Error(`Invalid Base58 character: ${encoded[i]}`)
		}
		const index = BASE58_DECODE_MAP[charCode]
		if (index === 255) {
			throw new Error(`Invalid Base58 character: ${encoded[i]}`)
		}
		decoded = decoded * 58n + BigInt(index)
	}

	// Overflow check: UUID is 128 bits max
	if (decoded > 0xffff_ffff_ffff_ffff_ffff_ffff_ffff_ffffn) {
		throw new Error("Invalid Base58: value exceeds 128-bit UUID range")
	}

	const hex = decoded.toString(16).padStart(32, "0")

	// Postcondition: padStart should always produce exactly 32 chars given the overflow check above
	if (hex.length !== 32) {
		throw new Error(`decodeBase58: internal error — produced ${hex.length}-char hex, expected 32`)
	}

	return hex
}

/**
 * Check if a string has valid Base58 syntax (alphabet + length ≤ 22).
 *
 * This checks syntax only — it does NOT verify the decoded value fits in
 * 128 bits. A 22-char string of high-value digits can pass this check but
 * overflow on decode. Use decodeBase58() for full validation.
 */
export function isBase58(value: string): boolean {
	if (value.length === 0 || value.length > MAX_BASE58_LENGTH) return false
	for (let i = 0; i < value.length; i++) {
		const charCode = value.charCodeAt(i)
		if (charCode >= 128 || BASE58_DECODE_MAP[charCode] === 255) return false
	}
	return true
}
