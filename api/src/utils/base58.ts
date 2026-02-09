/**
 * Base58 encoding/decoding for UUIDs.
 *
 * Ported from indexer_utils/src/id.rs to produce identical output.
 * Uses the Bitcoin Base58 alphabet (no 0, O, I, l).
 *
 * UUIDs are 128-bit values. Base58-encoded, they produce ~22 character strings
 * (variable length — leading zero bytes produce shorter output, no padding).
 */

const BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"

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
 */
export function encodeBase58(dashlessHex: string): string {
	let remainder = hexToBigInt(dashlessHex)
	if (remainder === 0n) return ""

	const chars: string[] = []
	while (remainder > 0n) {
		const mod = Number(remainder % 58n)
		chars.push(BASE58_ALPHABET[mod])
		remainder /= 58n
	}

	chars.reverse()
	return chars.join("")
}

/**
 * Decode a Base58 string to a dashless hex string (32 chars, zero-padded).
 *
 * Matches the Rust `decode_base58_to_uuid` implementation exactly.
 *
 * @throws if the input contains invalid Base58 characters or is empty
 */
export function decodeBase58(encoded: string): string {
	if (encoded.length === 0) {
		throw new Error("Invalid Base58: empty string")
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

	return decoded.toString(16).padStart(32, "0")
}

/**
 * Check if a string is a valid Base58-encoded UUID.
 */
export function isBase58(value: string): boolean {
	if (value.length === 0 || value.length > 22) return false
	for (let i = 0; i < value.length; i++) {
		const charCode = value.charCodeAt(i)
		if (charCode >= 128 || BASE58_DECODE_MAP[charCode] === 255) return false
	}
	return true
}

function hexToBigInt(hex: string): bigint {
	return BigInt(`0x${hex}`)
}
