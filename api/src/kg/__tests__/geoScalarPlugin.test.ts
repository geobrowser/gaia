import { Kind } from "graphql/language"
import { describe, expect, it } from "vitest"
import { GeoPointScalar, GeoRectScalar } from "../geoScalarPlugin"

describe("GeoPointScalar", () => {
	it("has descriptive name and description", () => {
		expect(GeoPointScalar.name).toBe("GeoPoint")
		expect(GeoPointScalar.description).toContain("WGS84")
		expect(GeoPointScalar.description).toContain("lat,lon")
	})

	it("serializes values as-is", () => {
		expect(GeoPointScalar.serialize("40.7128,-74.0060")).toBe("40.7128,-74.0060")
		expect(GeoPointScalar.serialize("40.7128,-74.0060,10.5")).toBe("40.7128,-74.0060,10.5")
	})

	it("parses string values", () => {
		expect(GeoPointScalar.parseValue("40.7128,-74.0060")).toBe("40.7128,-74.0060")
	})

	it("rejects non-string values", () => {
		expect(() => GeoPointScalar.parseValue(123)).toThrow("GeoPoint must be a string")
		expect(() => GeoPointScalar.parseValue({ lat: 40, lon: -74 })).toThrow(
			"GeoPoint must be a string",
		)
	})

	it("parses string literals", () => {
		expect(GeoPointScalar.parseLiteral({ kind: Kind.STRING, value: "40.7128,-74.0060" })).toBe(
			"40.7128,-74.0060",
		)
	})

	it("rejects non-string literals", () => {
		expect(() => GeoPointScalar.parseLiteral({ kind: Kind.INT, value: "123" } as any)).toThrow(
			"GeoPoint must be a string",
		)
	})
})

describe("GeoRectScalar", () => {
	it("has descriptive name and description", () => {
		expect(GeoRectScalar.name).toBe("GeoRect")
		expect(GeoRectScalar.description).toContain("WGS84")
		expect(GeoRectScalar.description).toContain("bounding box")
		expect(GeoRectScalar.description).toContain("min_lat,min_lon,max_lat,max_lon")
	})

	it("serializes values as-is", () => {
		expect(GeoRectScalar.serialize("24.5,-125.0,49.4,-66.9")).toBe("24.5,-125.0,49.4,-66.9")
	})

	it("parses string values", () => {
		expect(GeoRectScalar.parseValue("24.5,-125.0,49.4,-66.9")).toBe("24.5,-125.0,49.4,-66.9")
	})

	it("rejects non-string values", () => {
		expect(() => GeoRectScalar.parseValue(123)).toThrow("GeoRect must be a string")
		expect(() =>
			GeoRectScalar.parseValue({ minLat: 24.5, minLon: -125, maxLat: 49.4, maxLon: -66.9 }),
		).toThrow("GeoRect must be a string")
	})

	it("parses string literals", () => {
		expect(
			GeoRectScalar.parseLiteral({ kind: Kind.STRING, value: "24.5,-125.0,49.4,-66.9" }),
		).toBe("24.5,-125.0,49.4,-66.9")
	})

	it("rejects non-string literals", () => {
		expect(() => GeoRectScalar.parseLiteral({ kind: Kind.INT, value: "123" } as any)).toThrow(
			"GeoRect must be a string",
		)
	})
})
