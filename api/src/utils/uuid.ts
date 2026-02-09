/**
 * UUID validation and normalization utilities.
 *
 * The canonical UUID format in this codebase is **dashed lowercase hex** (36 chars),
 * matching PostgreSQL's native UUID representation. This eliminates conversion at
 * DB boundaries — what PostgreSQL returns is what we store internally.
 *
 * **Input boundaries (API params, GraphQL variables, request bodies):**
 *   Use `fromBase58()` / `isValidBase58Id()` — accepts Base58 only.
 *
 * **Internal conversions (DB rows, system constants, test fixtures):**
 *   Use `toUuid()` / `isValidUuid()` — accepts dashed hex, dashless hex, or Base58.
 *
 * **Output boundaries (API responses, GraphQL serialize):**
 *   Use `toBase58()` / `uuidToBase58()` — encodes to Base58.
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
	if (dashless.length !== 32) {
		throw new Error(`insertDashes: expected 32-char input, got ${dashless.length}`)
	}
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

	throw new Error(`Invalid UUID format (length=${trimmed.length})`)
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
 * Parse a Base58-encoded ID to a Uuid.
 *
 * This is the **input boundary** function: it accepts Base58 only and rejects
 * dashed hex and dashless hex. Use this for all external-facing inputs (REST
 * params, GraphQL variables, request bodies).
 *
 * For internal conversions (DB rows, system constants) that are already in
 * dashed hex, use `toUuid()` instead.
 *
 * @throws if the input is not valid Base58 or decodes to a value outside 128-bit range
 */
export function fromBase58(value: string): Uuid {
	const trimmed = value.trim()
	if (!isBase58(trimmed)) {
		throw new Error(`Invalid Base58 ID (length=${trimmed.length})`)
	}
	return insertDashes(decodeBase58(trimmed))
}

/**
 * Try to parse a Base58-encoded ID, returning null on failure.
 *
 * This is the preferred input boundary function: it validates and parses in a
 * single pass (one `decodeBase58` call), unlike `isValidBase58Id()` + `fromBase58()`
 * which would decode twice.
 *
 * @returns Uuid on success, null on invalid input
 */
export function tryFromBase58(value: string): Uuid | null {
	try {
		return fromBase58(value)
	} catch {
		return null
	}
}

/**
 * Validate that a string is a valid Base58-encoded ID.
 *
 * Returns true only for Base58 input. Dashed hex and dashless hex are rejected.
 * Use at input boundaries where only Base58 is accepted.
 *
 * Prefer `tryFromBase58()` when you need both validation and the parsed value
 * to avoid decoding twice.
 */
export function isValidBase58Id(value: string): boolean {
	return tryFromBase58(value) !== null
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

/**
 * Convenience: parse any UUID format (dashed hex, dashless hex, Base58) and
 * encode to Base58 in one step. Use at serialization boundaries where the
 * source format is unknown (e.g. raw DB strings, OpenSearch fields).
 *
 * @param value - A UUID in any accepted format
 * @returns Base58-encoded string
 */
export function uuidToBase58(value: string): string {
	return toBase58(toUuid(value))
}
