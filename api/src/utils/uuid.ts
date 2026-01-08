/**
 * UUID validation utilities.
 */

/**
 * UUID regex pattern for validation.
 */
const UUID_DASHED_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
const UUID_UNDASHED_PATTERN = /^[0-9a-f]{32}$/i

/**
 * Normalizes a UUID string to an undashed, lowercase representation.
 *
 * Accepts:
 * - dashed UUIDs: `550e8400-e29b-41d4-a716-446655440000`
 * - undashed UUIDs: `550e8400e29b41d4a716446655440000`
 *
 * @throws if the input is not a valid UUID in either form
 */
export function normalizeUuid(value: string): string {
	const trimmed = value.trim()
	if (UUID_UNDASHED_PATTERN.test(trimmed)) return trimmed.toLowerCase()
	if (UUID_DASHED_PATTERN.test(trimmed)) return trimmed.replaceAll("-", "").toLowerCase()
	throw new Error(`Invalid UUID: ${value}`)
}

/**
 * Validates if a string is a valid UUID.
 *
 * @param value - The string to validate
 * @returns true if the string is a valid UUID, false otherwise
 *
 * @example
 * ```typescript
 * isValidUuid("550e8400-e29b-41d4-a716-446655440000"); // true
 * isValidUuid("550e8400e29b41d4a716446655440000"); // true
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
