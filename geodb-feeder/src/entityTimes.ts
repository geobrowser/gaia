/**
 * An entity's createdAt / updatedAt as two reserved geo-db properties (geo-db docs/api/10 section 5.6).
 *
 * Both ids are fixed by name, minted here and mirrored as literals in `api/src/geodb/card.ts`
 * (`ENTITY_TIME_PROPERTIES`); `entityTimes.test.ts` and the api's card test pin the two sides to each other.
 */
import {conformingId} from "./ids.js"

export const CREATED_AT_PROPERTY = conformingId("reserved-property", "createdAt")
export const UPDATED_AT_PROPERTY = conformingId("reserved-property", "updatedAt")

/**
 * A block time in unix seconds as geo-db's instant (microseconds since the epoch), or null where the edit carried
 * none (or nonsense), so the edit writes no times.
 */
export function blockInstant(seconds: number | null | undefined): string | null {
	if (seconds == null || !Number.isSafeInteger(seconds) || seconds < 0) return null
	return `${BigInt(seconds) * 1_000_000n}`
}

/** `set` of one reserved time on an entity. */
export const timeIntent = (entity: string, property: string, instant: string): string =>
	`.{ .set = .{ .entity = "${entity}", .property = "${property}", .value = .{ .instant = "${instant}" } } }`
