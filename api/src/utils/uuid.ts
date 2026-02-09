/**
 * UUID validation and normalization utilities.
 *
 * The canonical UUID format in this codebase is dashless lowercase hex (32 chars).
 * PostgreSQL returns dashed UUIDs, GRC-20 SystemIds are dashed, and clients may
 * send either format. All UUIDs are normalized at I/O boundaries (HTTP input,
 * DB row mapping, binary Id conversion) so interior code never needs to worry
 * about format.
 *
 * Accepted input formats (disambiguation order):
 *   1. Dashed hex (36 chars):   `550e8400-e29b-41d4-a716-446655440000`
 *   2. Dashless hex (32 chars): `550e8400e29b41d4a716446655440000`
 *   3. Base58 (≤22 chars):      `BDuZwkjCg3nPWMDshoYtpS`
 *
 * Hex always wins when ambiguous (e.g. a 32-char hex-only string is hex, not Base58).
 *
 * The `NormalizedUuid` branded type provides compile-time safety: the compiler
 * will flag any place a raw `string` is used where a normalized UUID is expected.
 */

import {decodeBase58, isBase58} from "./base58"

/**
 * UUID regex pattern for validation.
 */
const UUID_DASHED_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
const UUID_UNDASHED_PATTERN = /^[0-9a-f]{32}$/i

/**
 * A UUID that has been normalized to dashless lowercase hex (32 chars).
 *
 * This branded type prevents accidentally mixing raw (potentially dashed)
 * UUID strings with normalized ones. Use `normalizeUuid()` to produce values
 * of this type at I/O boundaries.
 */
export type NormalizedUuid = string & {readonly __brand: "NormalizedUuid"}

/**
 * Normalizes a UUID string to an undashed, lowercase representation.
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
export function normalizeUuid(value: string): NormalizedUuid {
	const trimmed = value.trim()
	if (UUID_UNDASHED_PATTERN.test(trimmed)) return trimmed.toLowerCase() as NormalizedUuid
	if (UUID_DASHED_PATTERN.test(trimmed)) return trimmed.replaceAll("-", "").toLowerCase() as NormalizedUuid

	// Try Base58 last — only if it doesn't look like hex
	if (isBase58(trimmed)) {
		try {
			return decodeBase58(trimmed)
		} catch {
			// Fall through to throw below
		}
	}

	throw new Error(`Invalid UUID: ${value}`)
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
		normalizeUuid(value)
		return true
	} catch {
		return false
	}
}

/**
 * Converts a UUID to dashed format (xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx).
 * PostgreSQL returns UUIDs in dashed format, so this is useful for consistent comparisons.
 *
 * @param value - A valid UUID string (dashed, undashed, or Base58-encoded)
 * @returns The UUID in dashed lowercase format
 * @throws if the input is not a valid UUID
 *
 * @example
 * ```typescript
 * toDashedUuid("550e8400e29b41d4a716446655440000"); // "550e8400-e29b-41d4-a716-446655440000"
 * toDashedUuid("550e8400-e29b-41d4-a716-446655440000"); // "550e8400-e29b-41d4-a716-446655440000"
 * toDashedUuid("BDuZwkjCg3nPWMDshoYtpS"); // "52c8b540-e249-4e1b-babc-c8eac0acaa23"
 * ```
 */
export function toDashedUuid(value: string): string {
	const normalized = normalizeUuid(value)
	return `${normalized.slice(0, 8)}-${normalized.slice(8, 12)}-${normalized.slice(12, 16)}-${normalized.slice(16, 20)}-${normalized.slice(20)}`
}
