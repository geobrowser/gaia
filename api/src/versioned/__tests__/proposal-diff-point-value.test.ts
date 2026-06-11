/**
 * Regression test for the versioned proposal-diff point/rect value handling.
 *
 * Bug: `/versioned/proposals/:id/diff` returned 500 for any edit containing a
 * POINT value. `propertyValueToVersionedValue` read the coordinates from
 * `value.value`, but GRC-20 decodes point/rect with their fields at the *top
 * level* of the value ({type: "point", lat, lon, alt?}). `value.value` was
 * therefore `undefined`, and accessing `.alt` on it threw a TypeError that
 * surfaced as a 500.
 *
 * These are pure-function unit tests — no database required, so they always run
 * (unlike the DB-gated proposal-diff-edit-flow integration suite, whose
 * "all value types" fixture covered text/bool/int/float but not point).
 */

import type {PropertyValue} from "@geoprotocol/grc-20"
import {describe, expect, it} from "vitest"
import {propertyValueToVersionedValue} from "../proposal-diff"
import {normalizeUuid} from "../../utils/uuid"

const spaceId = normalizeUuid("20000000-0001-4000-8000-000000000001")
const propertyId = "20000000-0004-4000-8000-000000000050"

// `pv` is typed against the real SDK `PropertyValue`, so if a future grc-20
// version moves point coords back under `value.value` the test stops compiling.
function pv(value: PropertyValue["value"]): {property: any; value: PropertyValue["value"]} {
	return {property: propertyId as any, value}
}

describe("propertyValueToVersionedValue — point/rect", () => {
	it("does not throw and serializes a point value as 'lat,lon'", () => {
		const result = propertyValueToVersionedValue(pv({type: "point", lat: 37.7749, lon: -122.4194}), spaceId)

		// Matches the indexer's stored format (kg-indexer handlers/edits.rs):
		// "lat,lon", so before/after compare equal instead of showing a spurious diff.
		expect(result.point).toBe("37.7749,-122.4194")
	})

	it("includes altitude when present: 'lat,lon,alt'", () => {
		const result = propertyValueToVersionedValue(
			pv({type: "point", lat: 37.7749, lon: -122.4194, alt: 10.5}),
			spaceId,
		)

		expect(result.point).toBe("37.7749,-122.4194,10.5")
	})

	it("does not throw and serializes a rect value as 'minLat,minLon,maxLat,maxLon'", () => {
		const result = propertyValueToVersionedValue(
			pv({type: "rect", minLat: 1, minLon: 2, maxLat: 3, maxLon: 4}),
			spaceId,
		)

		expect(result.rect).toBe("1,2,3,4")
	})
})
