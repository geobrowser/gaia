import type {SQL} from "drizzle-orm"
import {expect, vi} from "vitest"

// Mock database schema objects that match the real schema structure
export const mockEntities = {
	id: {name: "id"},
}

export const mockValues = {
	id: {name: "id"},
	propertyId: {name: "property_id"},
	entityId: {name: "entity_id"},
	spaceId: {name: "space_id"},
	value: {name: "value"},
	language: {name: "language"},
	format: {name: "format"},
	unit: {name: "unit"},
	timezone: {name: "timezone"},
	hasDate: {name: "has_date"},
	hasTime: {name: "has_time"},
}

export const mockRelations = {
	id: {name: "id"},
	entityId: {name: "entity_id"},
	typeId: {name: "type_id"},
	fromEntityId: {name: "from_entity_id"},
	fromSpaceId: {name: "from_space_id"},
	fromVersionId: {name: "from_version_id"},
	toEntityId: {name: "to_entity_id"},
	toSpaceId: {name: "to_space_id"},
	toVersionId: {name: "to_version_id"},
	position: {name: "position"},
	spaceId: {name: "space_id"},
	verified: {name: "verified"},
}

// Mock the schema module
export const mockSchemaModule = () => {
	vi.mock("../../services/storage/schema", () => ({
		entities: mockEntities,
		values: mockValues,
		relations: mockRelations,
	}))
}

// Helper to extract SQL string from Drizzle SQL object
export const extractSQLString = (sqlObj: SQL): string => {
	if (!sqlObj) return ""

	let sqlString = ""
	const chunks = sqlObj.queryChunks

	for (let i = 0; i < chunks.length; i++) {
		const chunk = chunks[i]
		if (typeof chunk === "string") {
			sqlString += chunk
		} else if (chunk && typeof chunk === "object" && "queryChunks" in chunk) {
			sqlString += extractSQLString(chunk as SQL)
		} else {
			// Parameter placeholder
			sqlString += `$${i + 1}`
		}
	}

	return sqlString
}

// Helper to extract parameters from Drizzle SQL object
export const extractSQLParams = (sqlObj: SQL): unknown[] => {
	if (!sqlObj) return []
	return (sqlObj as SQL & { params?: unknown[] })?.params || []
}

// Test data factories
export const createTextFilter = (
	options: Partial<{
		is: string
		contains: string
		startsWith: string
		endsWith: string
		exists: boolean
		NOT: unknown
	}> = {},
) => ({
	...options,
})

export const createNumberFilter = (
	options: Partial<{
		is: number
		lessThan: number
		lessThanOrEqual: number
		greaterThan: number
		greaterThanOrEqual: number
		exists: boolean
		NOT: unknown
	}> = {},
) => ({
	...options,
})

export const createCheckboxFilter = (
	options: Partial<{
		is: boolean
		exists: boolean
	}> = {},
) => ({
	...options,
})

export const createPointFilter = (
	options: Partial<{
		is: [number, number]
		exists: boolean
	}> = {},
) => ({
	...options,
})

export const createRelationFilter = (
	options: Partial<{
		typeId: string
		fromEntityId: string
		toEntityId: string
		spaceId: string
	}> = {},
) => ({
	...options,
})

export const createValueFilter = (
	property: string,
	options: Partial<{
		text: unknown
		number: unknown
		checkbox: unknown
		point: unknown
	}> = {},
) => ({
	property,
	...options,
})

export const createEntityFilter = (
	options: Partial<{
		AND: unknown[]
		OR: unknown[]
		NOT: unknown
		value: unknown
		fromRelation: unknown
		toRelation: unknown
	}> = {},
) => ({
	...options,
})

// SQL assertion helpers
export const expectSQLToContain = (sqlObj: SQL, expected: string) => {
	const sqlString = extractSQLString(sqlObj)
	expect(sqlString.toLowerCase()).toContain(expected.toLowerCase())
}

export const expectSQLToContainAll = (sqlObj: SQL, expectedStrings: string[]) => {
	const sqlString = extractSQLString(sqlObj).toLowerCase()
	for (const expected of expectedStrings) {
		expect(sqlString).toContain(expected.toLowerCase())
	}
}

export const expectSQLParamsToContain = (sqlObj: SQL, expectedParam: unknown) => {
	const params = extractSQLParams(sqlObj)
	expect(params).toContain(expectedParam)
}

// Mock Effect framework for testing
export const mockEffect = () => {
	vi.mock("effect", () => ({
		Effect: {
			gen: vi.fn((fn) => ({
				pipe: vi.fn(),
				run: vi.fn(),
			})),
		},
		Layer: {
			effect: vi.fn(),
			provide: vi.fn(),
			mergeAll: vi.fn(),
		},
	}))
}

// Mock storage service
export const mockStorage = () => {
	const mockClient = {
		query: {
			entities: {
				findMany: vi.fn(),
				findFirst: vi.fn(),
			},
			values: {
				findMany: vi.fn(),
				findFirst: vi.fn(),
			},
			relations: {
				findMany: vi.fn(),
				findFirst: vi.fn(),
			},
		},
		select: vi.fn(() => ({
			from: vi.fn(() => ({
				where: vi.fn(),
			})),
		})),
	}

	const mockStorage = {
		use: vi.fn((fn) => fn(mockClient)),
	}

	vi.mock("../../services/storage/storage", () => ({
		Storage: mockStorage,
	}))

	return {mockClient, mockStorage}
}

// Database column name mappings (camelCase to snake_case)
export const columnMappings = {
	propertyId: "property_id",
	entityId: "entity_id",
	spaceId: "space_id",
	typeId: "type_id",
	fromEntityId: "from_entity_id",
	fromSpaceId: "from_space_id",
	fromVersionId: "from_version_id",
	toEntityId: "to_entity_id",
	toSpaceId: "to_space_id",
	toVersionId: "to_version_id",
	hasDate: "has_date",
	hasTime: "has_time",
}

// Test scenarios for comprehensive testing
export const testScenarios = {
	textFilters: [
		{name: "exact match", filter: {is: "exact value"}},
		{name: "contains", filter: {contains: "partial"}},
		{name: "starts with", filter: {startsWith: "prefix"}},
		{name: "ends with", filter: {endsWith: "suffix"}},
		{name: "exists", filter: {exists: true}},
		{name: "not exists", filter: {exists: false}},
		{name: "NOT filter", filter: {NOT: {is: "excluded"}}},
	],
	numberFilters: [
		{name: "equals", filter: {is: 42}},
		{name: "less than", filter: {lessThan: 100}},
		{name: "less than or equal", filter: {lessThanOrEqual: 50}},
		{name: "greater than", filter: {greaterThan: 0}},
		{name: "greater than or equal", filter: {greaterThanOrEqual: 1}},
		{name: "exists", filter: {exists: true}},
		{name: "NOT filter", filter: {NOT: {is: 0}}},
	],
	checkboxFilters: [
		{name: "is true", filter: {is: true}},
		{name: "is false", filter: {is: false}},
		{name: "exists", filter: {exists: true}},
	],
	pointFilters: [
		{name: "exact coordinates", filter: {is: [40.7128, -74.006]}},
		{name: "exists", filter: {exists: true}},
	],
	relationFilters: [
		{name: "by type", filter: {typeId: "type-123"}},
		{name: "by from entity", filter: {fromEntityId: "entity-456"}},
		{name: "by to entity", filter: {toEntityId: "entity-789"}},
		{name: "by space", filter: {spaceId: "space-abc"}},
		{name: "combined", filter: {typeId: "type-123", spaceId: "space-abc"}},
	],
}
