import {describe, expect, it} from "vitest"
import {type EntityFilter, buildEntityWhere} from "../filters"

describe("Entity Filter Tests", () => {
	describe("Basic Functionality", () => {
		it("should return undefined for null filter", () => {
			const result = buildEntityWhere(null)
			expect(result).toBeUndefined()
		})

		it("should return undefined for empty filter", () => {
			const result = buildEntityWhere({})
			expect(result).toBeUndefined()
		})

		it("should return a SQL object for valid filters", () => {
			const filter: EntityFilter = {
				value: {
					property: "name",
					text: {is: "test"},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
			expect(typeof result).toBe("object")
			expect(result?.constructor.name).toBe("SQL")
		})
	})

	describe("Text Filters", () => {
		it('should handle text "is" filter', () => {
			const filter: EntityFilter = {
				value: {
					property: "name",
					text: {is: "John Doe"},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
			expect(result?.constructor.name).toBe("SQL")
		})

		it('should handle text "contains" filter', () => {
			const filter: EntityFilter = {
				value: {
					property: "description",
					text: {contains: "important"},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it('should handle text "startsWith" filter', () => {
			const filter: EntityFilter = {
				value: {
					property: "title",
					text: {startsWith: "Project"},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it('should handle text "endsWith" filter', () => {
			const filter: EntityFilter = {
				value: {
					property: "filename",
					text: {endsWith: ".pdf"},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it('should handle text "exists" filter', () => {
			const filter: EntityFilter = {
				value: {
					property: "optional_field",
					text: {exists: true},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle text NOT filter", () => {
			const filter: EntityFilter = {
				value: {
					property: "status",
					text: {NOT: {is: "deleted"}},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})
	})

	describe("Number Filters", () => {
		it('should handle number "is" filter', () => {
			const filter: EntityFilter = {
				value: {
					property: "age",
					number: {is: 25},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it('should handle number "greaterThan" filter', () => {
			const filter: EntityFilter = {
				value: {
					property: "score",
					number: {greaterThan: 100},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it('should handle number "lessThan" filter', () => {
			const filter: EntityFilter = {
				value: {
					property: "count",
					number: {lessThan: 50},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it('should handle number "exists" filter', () => {
			const filter: EntityFilter = {
				value: {
					property: "weight",
					number: {exists: true},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})
	})

	describe("Checkbox Filters", () => {
		it("should handle checkbox true", () => {
			const filter: EntityFilter = {
				value: {
					property: "active",
					checkbox: {is: true},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle checkbox false", () => {
			const filter: EntityFilter = {
				value: {
					property: "published",
					checkbox: {is: false},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle checkbox exists", () => {
			const filter: EntityFilter = {
				value: {
					property: "verified",
					checkbox: {exists: true},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})
	})

	describe("Point Filters", () => {
		it("should handle point coordinates", () => {
			const filter: EntityFilter = {
				value: {
					property: "location",
					point: {is: [40.7128, -74.006]},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle point exists", () => {
			const filter: EntityFilter = {
				value: {
					property: "coordinates",
					point: {exists: true},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})
	})

	describe("Relation Filters", () => {
		it("should handle fromRelation filter", () => {
			const filter: EntityFilter = {
				fromRelation: {
					typeId: "manages",
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
			expect(result?.constructor.name).toBe("SQL")
		})

		it("should handle toRelation filter", () => {
			const filter: EntityFilter = {
				toRelation: {
					typeId: "reports-to",
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle fromRelation with multiple conditions", () => {
			const filter: EntityFilter = {
				fromRelation: {
					typeId: "member-of",
					toEntityId: "org-123",
					spaceId: "space-456",
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle empty fromRelation filter", () => {
			const filter: EntityFilter = {
				fromRelation: {},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle both fromRelation and toRelation", () => {
			const filter: EntityFilter = {
				fromRelation: {
					typeId: "manages",
				},
				toRelation: {
					typeId: "reports-to",
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle fromRelation with fromEntityId", () => {
			const filter: EntityFilter = {
				fromRelation: {
					fromEntityId: "user-123",
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle toRelation with all fields", () => {
			const filter: EntityFilter = {
				toRelation: {
					typeId: "assigned-to",
					fromEntityId: "manager-456",
					spaceId: "project-space",
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})
	})

	describe("Logical Operators", () => {
		it("should handle AND filters", () => {
			const filter: EntityFilter = {
				AND: [
					{
						value: {
							property: "status",
							text: {is: "active"},
						},
					},
					{
						value: {
							property: "type",
							text: {is: "user"},
						},
					},
				],
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle OR filters", () => {
			const filter: EntityFilter = {
				OR: [
					{
						value: {
							property: "role",
							text: {is: "admin"},
						},
					},
					{
						value: {
							property: "role",
							text: {is: "moderator"},
						},
					},
				],
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle NOT filters", () => {
			const filter: EntityFilter = {
				NOT: {
					value: {
						property: "status",
						text: {is: "banned"},
					},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle empty AND array", () => {
			const filter: EntityFilter = {
				AND: [],
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeUndefined()
		})

		it("should handle empty OR array", () => {
			const filter: EntityFilter = {
				OR: [],
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeUndefined()
		})
	})

	describe("Complex Combinations", () => {
		it("should combine value and relation filters", () => {
			const filter: EntityFilter = {
				value: {
					property: "status",
					text: {is: "active"},
				},
				fromRelation: {
					typeId: "member-of",
					spaceId: "org-123",
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle deeply nested filters", () => {
			const filter: EntityFilter = {
				AND: [
					{
						OR: [
							{
								value: {
									property: "type",
									text: {is: "admin"},
								},
							},
							{
								value: {
									property: "type",
									text: {is: "moderator"},
								},
							},
						],
					},
					{
						NOT: {
							value: {
								property: "status",
								text: {is: "suspended"},
							},
						},
					},
					{
						fromRelation: {
							typeId: "member-of",
						},
					},
				],
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle all filter types together", () => {
			const filter: EntityFilter = {
				AND: [
					{
						value: {
							property: "age",
							number: {greaterThan: 18},
						},
					},
					{
						value: {
							property: "active",
							checkbox: {is: true},
						},
					},
				],
				fromRelation: {
					typeId: "member-of",
					spaceId: "org-123",
				},
				NOT: {
					toRelation: {
						typeId: "blocked-by",
					},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})
	})

	describe("Edge Cases", () => {
		it("should handle empty strings", () => {
			const filter: EntityFilter = {
				value: {
					property: "",
					text: {is: ""},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle zero values", () => {
			const filter: EntityFilter = {
				value: {
					property: "count",
					number: {is: 0},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle negative numbers", () => {
			const filter: EntityFilter = {
				value: {
					property: "balance",
					number: {lessThan: -100},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle decimal numbers", () => {
			const filter: EntityFilter = {
				value: {
					property: "rating",
					number: {greaterThan: 4.5},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle special characters", () => {
			const specialText = "'; DROP TABLE users; --"
			const filter: EntityFilter = {
				value: {
					property: "comment",
					text: {contains: specialText},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle Unicode characters", () => {
			const unicodeText = "🎉 Hello 世界"
			const filter: EntityFilter = {
				value: {
					property: "message",
					text: {is: unicodeText},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})

		it("should handle very large numbers", () => {
			const largeNumber = 9007199254740991
			const filter: EntityFilter = {
				value: {
					property: "id",
					number: {is: largeNumber},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
		})
	})

	describe("SQL Structure Validation", () => {
		it("should return SQL object with queryChunks", () => {
			const filter: EntityFilter = {
				value: {
					property: "name",
					text: {is: "test"},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
			expect(result?.queryChunks).toBeDefined()
			expect(Array.isArray(result?.queryChunks)).toBe(true)
		})

		it("should return a valid SQL object", () => {
			const filter: EntityFilter = {
				value: {
					property: "name",
					text: {is: "test"},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
			expect(result?.constructor.name).toBe("SQL")
		})

		it("should generate proper SQL structure", () => {
			const filter: EntityFilter = {
				value: {
					property: "name",
					text: {is: "test"},
				},
			}

			const result = buildEntityWhere(filter)
			expect(result).toBeDefined()
			expect(typeof result).toBe("object")
			expect(result?.constructor.name).toBe("SQL")
		})
	})

	describe("Filter Type Coverage", () => {
		it("should handle all text filter operators", () => {
			const operators = ["is", "contains", "startsWith", "endsWith", "exists"]

			for (const op of operators) {
				const filter: EntityFilter = {
					value: {
						property: "test",
						text: {[op]: op === "exists" ? true : "value"},
					},
				}

				const result = buildEntityWhere(filter)
				expect(result).toBeDefined()
			}
		})

		it("should handle all number filter operators", () => {
			const operators = ["is", "lessThan", "lessThanOrEqual", "greaterThan", "greaterThanOrEqual", "exists"]

			for (const op of operators) {
				const filter: EntityFilter = {
					value: {
						property: "test",
						number: {[op]: op === "exists" ? true : 42},
					},
				}

				const result = buildEntityWhere(filter)
				expect(result).toBeDefined()
			}
		})

		it("should handle all relation filter fields", () => {
			const fields = ["typeId", "fromEntityId", "toEntityId", "spaceId"]

			for (const field of fields) {
				const filter: EntityFilter = {
					fromRelation: {[field]: "test-value"},
				}

				const result = buildEntityWhere(filter)
				expect(result).toBeDefined()
			}
		})
	})
})
