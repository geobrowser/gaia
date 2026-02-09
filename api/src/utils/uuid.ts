/**
 * UUID validation and normalization utilities.
 *
 * The canonical UUID format in this codebase is **dashed lowercase hex** (36 chars),
 * matching PostgreSQL's native UUID representation. This eliminates conversion at
 * DB boundaries — what PostgreSQL returns is what we store internally.
 *
 * Accepted input formats (disambiguation order):
 *   1. Dashed hex (36 chars):   `550e8400-e29b-41d4-a716-446655440000`
 *   2. Dashless hex (32 chars): `550e8400e29b41d4a716446655440000`
 *   3. Base58 (≤22 chars):      `BDuZwkjCg3nPWMDshoYtpS`
 *
 * Hex always wins when ambiguous (e.g. a 32-char hex-only string is hex, not Base58).
 *
 * Output format is Base58 for API responses. Use `toBase58()` at serialization boundaries.
 *
 * The `Uuid` branded type provides compile-time safety: the compiler will flag any
 * place a raw `string` is used where a validated UUID is expected.
 */

import {decodeBase58, encodeBase58, isBase58} from "./base58"

/**
 * UUID regex pattern for validation.
 */
const UUID_DASHED_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
const UUID_UNDASHED_PATTERN = /^[0-9a-f]{32}$/i

/**
 * A validated UUID in dashed lowercase hex format (36 chars).
 *
 * This branded type prevents accidentally mixing raw (potentially invalid)
 * strings with validated UUIDs. Use `toUuid()` to produce values of this type
 * at I/O boundaries.
 */
export type Uuid = string & {readonly __brand: "Uuid"}

/**
 * Insert dashes into a 32-char dashless hex string to produce a Uuid.
 */
function insertDashes(dashless: string): Uuid {
	const result =
		`${dashless.slice(0, 8)}-${dashless.slice(8, 12)}-${dashless.slice(12, 16)}-${dashless.slice(16, 20)}-${dashless.slice(20)}` as Uuid
	if (result.length !== 36) {
		throw new Error(`insertDashes: expected 36-char result, got ${result.length}`)
	}
	return result
}

/**
 * Normalizes any UUID input to dashed lowercase hex.
 *
 * Accepts:
 * - dashed UUIDs: `550e8400-e29b-41d4-a716-446655440000`
 * - undashed UUIDs: `550e8400e29b41d4a716446655440000`
 * - Base58-encoded UUIDs: `BDuZwkjCg3nPWMDshoYtpS`
 *
 * Disambiguation order: dashed hex → dashless hex → Base58.
 * Hex wins when ambiguous.
 *
 * @throws if the input is not a valid UUID in any accepted format
 */
export function toUuid(value: string): Uuid {
	const trimmed = value.trim()

	// 1. Dashed hex (most common — what DB returns)
	if (UUID_DASHED_PATTERN.test(trimmed)) return trimmed.toLowerCase() as Uuid

	// 2. Dashless hex
	if (UUID_UNDASHED_PATTERN.test(trimmed)) return insertDashes(trimmed.toLowerCase())

	// 3. Base58 — if isBase58 accepts it, let decodeBase58 throw with its
	// specific error (e.g. "value exceeds 128-bit UUID range") rather than
	// swallowing it into a generic "Invalid UUID" message.
	if (isBase58(trimmed)) return insertDashes(decodeBase58(trimmed))

	throw new Error(`Invalid UUID format`)
}

/**
 * Validates if a string is a valid UUID (hex or Base58-encoded).
 *
 * @param value - The string to validate
 * @returns true if the string is a valid UUID, false otherwise
 *
 * @example
 * ```typescript
 * isValidUuid("550e8400-e29b-41d4-a716-446655440000"); // true
 * isValidUuid("550e8400e29b41d4a716446655440000"); // true
 * isValidUuid("BDuZwkjCg3nPWMDshoYtpS"); // true (Base58)
 * isValidUuid("invalid-uuid"); // false
 * ```
 */
export function isValidUuid(value: string): boolean {
	try {
		toUuid(value)
		return true
	} catch {
		return false
	}
}

/**
 * Encode a Uuid to Base58 for API output.
 *
 * NOTE: The zero UUID encodes to an empty string (Rust parity).
 * Zero UUIDs should not appear in production data.
 *
 * @param uuid - A validated Uuid (dashed hex)
 * @returns Base58-encoded string
 */
export function toBase58(uuid: Uuid): string {
	const dashless = uuid.replaceAll("-", "")
	if (dashless.length !== 32) {
		throw new Error(`toBase58: expected 36-char dashed UUID, got: "${uuid}"`)
	}
	return encodeBase58(dashless)
}
