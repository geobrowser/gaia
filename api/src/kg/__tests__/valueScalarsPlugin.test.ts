import {Kind} from "graphql/language"
import {describe, expect, it} from "vitest"
import {graphqlServer} from "../postgraphile"
import {
	BytesScalar,
	DateStringScalar,
	DateTimeStringScalar,
	GeoPointScalar,
	GeoRectScalar,
	LanguageTagScalar,
	TimeStringScalar,
} from "../valueScalarsPlugin"

// Helper to execute GraphQL queries against the yoga server
async function executeGraphQL(query: string, variables?: Record<string, unknown>) {
	const response = await graphqlServer.fetch(
		new Request("http://localhost/graphql", {
			method: "POST",
			headers: {"Content-Type": "application/json"},
			body: JSON.stringify({query, variables}),
		}),
		{},
	)
	return response.json()
}

// =============================================================================
// Scalar unit tests
// =============================================================================

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
		expect(() => GeoPointScalar.parseValue({lat: 40, lon: -74})).toThrow("GeoPoint must be a string")
	})

	it("parses string literals", () => {
		expect(GeoPointScalar.parseLiteral({kind: Kind.STRING, value: "40.7128,-74.0060"})).toBe("40.7128,-74.0060")
	})

	it("rejects non-string literals", () => {
		expect(() => GeoPointScalar.parseLiteral({kind: Kind.INT, value: "123"} as any)).toThrow(
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
		expect(() => GeoRectScalar.parseValue({minLat: 24.5, minLon: -125, maxLat: 49.4, maxLon: -66.9})).toThrow(
			"GeoRect must be a string",
		)
	})

	it("parses string literals", () => {
		expect(GeoRectScalar.parseLiteral({kind: Kind.STRING, value: "24.5,-125.0,49.4,-66.9"})).toBe(
			"24.5,-125.0,49.4,-66.9",
		)
	})

	it("rejects non-string literals", () => {
		expect(() => GeoRectScalar.parseLiteral({kind: Kind.INT, value: "123"} as any)).toThrow(
			"GeoRect must be a string",
		)
	})
})

describe("DateStringScalar", () => {
	it("has descriptive name and description", () => {
		expect(DateStringScalar.name).toBe("DateString")
		expect(DateStringScalar.description).toContain("YYYY-MM-DD")
		expect(DateStringScalar.description).toContain("ISO 8601")
	})

	it("parses and serializes string values", () => {
		expect(DateStringScalar.parseValue("2024-01-15")).toBe("2024-01-15")
		expect(DateStringScalar.serialize("2024-01-15")).toBe("2024-01-15")
	})

	it("rejects non-string values", () => {
		expect(() => DateStringScalar.parseValue(20240115)).toThrow("DateString must be a string")
	})
})

describe("DateTimeStringScalar", () => {
	it("has descriptive name and description", () => {
		expect(DateTimeStringScalar.name).toBe("DateTimeString")
		expect(DateTimeStringScalar.description).toContain("ISO 8601")
	})

	it("parses and serializes string values", () => {
		expect(DateTimeStringScalar.parseValue("2024-01-15T14:30:00")).toBe("2024-01-15T14:30:00")
		expect(DateTimeStringScalar.parseValue("2024-01-15T14:30:00+05:30")).toBe("2024-01-15T14:30:00+05:30")
	})

	it("rejects non-string values", () => {
		expect(() => DateTimeStringScalar.parseValue(new Date())).toThrow("DateTimeString must be a string")
	})
})

describe("TimeStringScalar", () => {
	it("has descriptive name and description", () => {
		expect(TimeStringScalar.name).toBe("TimeString")
		expect(TimeStringScalar.description).toContain("HH:MM:SS")
	})

	it("parses and serializes string values", () => {
		expect(TimeStringScalar.parseValue("14:30:00")).toBe("14:30:00")
		expect(TimeStringScalar.parseValue("14:30:00.123")).toBe("14:30:00.123")
	})

	it("rejects non-string values", () => {
		expect(() => TimeStringScalar.parseValue(143000)).toThrow("TimeString must be a string")
	})
})

describe("BytesScalar", () => {
	it("has descriptive name and description", () => {
		expect(BytesScalar.name).toBe("Bytes")
		expect(BytesScalar.description).toContain("Base64")
	})

	it("parses and serializes string values", () => {
		expect(BytesScalar.parseValue("SGVsbG8gV29ybGQ=")).toBe("SGVsbG8gV29ybGQ=")
	})

	it("rejects non-string values", () => {
		expect(() => BytesScalar.parseValue(Buffer.from("test"))).toThrow("Bytes must be a string")
	})
})

describe("LanguageTagScalar", () => {
	it("has descriptive name and description", () => {
		expect(LanguageTagScalar.name).toBe("LanguageTag")
		expect(LanguageTagScalar.description).toContain("BCP 47")
	})

	it("parses and serializes string values", () => {
		expect(LanguageTagScalar.parseValue("en")).toBe("en")
		expect(LanguageTagScalar.parseValue("en-US")).toBe("en-US")
		expect(LanguageTagScalar.parseValue("zh-Hans-CN")).toBe("zh-Hans-CN")
	})

	it("rejects non-string values", () => {
		expect(() => LanguageTagScalar.parseValue({lang: "en"})).toThrow("LanguageTag must be a string")
	})
})

// =============================================================================
// Schema integration tests
// =============================================================================

describe("ValueScalarsPlugin schema integration", () => {
	it("should expose all custom scalar types", async () => {
		const result = await executeGraphQL(`
			query IntrospectScalars {
				geoPoint: __type(name: "GeoPoint") { name kind }
				geoRect: __type(name: "GeoRect") { name kind }
				dateString: __type(name: "DateString") { name kind }
				dateTimeString: __type(name: "DateTimeString") { name kind }
				timeString: __type(name: "TimeString") { name kind }
				bytes: __type(name: "Bytes") { name kind }
				languageTag: __type(name: "LanguageTag") { name kind }
			}
		`)

		expect(result.errors).toBeUndefined()
		expect(result.data.geoPoint).toEqual({name: "GeoPoint", kind: "SCALAR"})
		expect(result.data.geoRect).toEqual({name: "GeoRect", kind: "SCALAR"})
		expect(result.data.dateString).toEqual({name: "DateString", kind: "SCALAR"})
		expect(result.data.dateTimeString).toEqual({name: "DateTimeString", kind: "SCALAR"})
		expect(result.data.timeString).toEqual({name: "TimeString", kind: "SCALAR"})
		expect(result.data.bytes).toEqual({name: "Bytes", kind: "SCALAR"})
		expect(result.data.languageTag).toEqual({name: "LanguageTag", kind: "SCALAR"})
	})

	it("should use custom scalars for Value type fields", async () => {
		const result = await executeGraphQL(`
			query IntrospectValueType {
				__type(name: "Value") {
					fields {
						name
						type {
							name
							kind
							ofType { name kind }
						}
					}
				}
			}
		`)

		expect(result.errors).toBeUndefined()

		const getFieldType = (name: string) => {
			const field = result.data.__type?.fields?.find((f: {name: string}) => f.name === name)
			return field?.type.name || field?.type.ofType?.name
		}

		expect(getFieldType("point")).toBe("GeoPoint")
		expect(getFieldType("rect")).toBe("GeoRect")
		expect(getFieldType("date")).toBe("DateString")
		expect(getFieldType("datetime")).toBe("DateTimeString")
		expect(getFieldType("time")).toBe("TimeString")
		expect(getFieldType("bytes")).toBe("Bytes")
		expect(getFieldType("language")).toBe("LanguageTag")
	})

	it("should use custom scalars for ValueVersion type fields", async () => {
		const result = await executeGraphQL(`
			query IntrospectValueVersionType {
				__type(name: "ValueVersion") {
					fields {
						name
						type {
							name
							kind
							ofType { name kind }
						}
					}
				}
			}
		`)

		expect(result.errors).toBeUndefined()

		const getFieldType = (name: string) => {
			const field = result.data.__type?.fields?.find((f: {name: string}) => f.name === name)
			return field?.type.name || field?.type.ofType?.name
		}

		expect(getFieldType("point")).toBe("GeoPoint")
		expect(getFieldType("rect")).toBe("GeoRect")
		expect(getFieldType("date")).toBe("DateString")
		expect(getFieldType("datetime")).toBe("DateTimeString")
		expect(getFieldType("time")).toBe("TimeString")
		expect(getFieldType("bytes")).toBe("Bytes")
		expect(getFieldType("language")).toBe("LanguageTag")
	})
})
