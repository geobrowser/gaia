# Entity Filter Tests

This directory contains comprehensive integration tests for the entity filtering system in the Gaia API.

## Overview

The filter tests verify that the GraphQL entity filters work correctly against a live database. These are integration tests that seed the database with test data, run filter queries, and verify the results.

## Test Structure

### Test Data Setup

Each test creates fresh test data with unique UUIDs to avoid conflicts:

- **3 Test Entities**: Each with different property values
- **Multiple Property Types**: Text, Number, Checkbox, Point, and Relations
- **Unique Space ID**: Ensures test isolation from existing database data

### Test Categories

#### Text Filters
- `is` - Exact text matching
- `contains` - Substring matching  
- `startsWith` - Prefix matching
- `endsWith` - Suffix matching
- `exists` - Property existence checking
- `NOT` - Negation of text conditions

#### Number Filters
- `is` - Exact number matching
- `greaterThan` / `lessThan` - Numeric comparisons
- `greaterThanOrEqual` / `lessThanOrEqual` - Inclusive comparisons
- `exists` - Numeric property existence (validates numeric format)
- `NOT` - Negation of number conditions

#### Checkbox Filters
- `is` - Boolean value matching (true/false)
- `exists` - Boolean property existence

#### Point Filters
- `is` - Exact coordinate matching `[x, y]`
- `exists` - Point property existence

#### Relation Filters
- `fromRelation` - Entities that have outgoing relations
- `toRelation` - Entities that have incoming relations
- Supports filtering by `typeId`, `fromEntityId`, `toEntityId`, `spaceId`

#### Complex Filters
- `AND` - Logical conjunction of multiple conditions
- `OR` - Logical disjunction of multiple conditions  
- `NOT` - Logical negation of conditions
- Nested combinations of the above

## Running the Tests

```bash
# Run all filter tests
bun test src/__tests__/filters.test.ts

# Run specific test categories
bun test src/__tests__/filters.test.ts --grep "Text Filters"
bun test src/__tests__/filters.test.ts --grep "Number Filters"
bun test src/__tests__/filters.test.ts --grep "Complex Filters"
```

## Test Data Schema

### Entities
```typescript
TEST_ENTITY_1_ID: "Entity One" 
- Text: "Hello World"
- Number: "42" 
- Checkbox: "true"
- Point: [1.0, 2.0]

TEST_ENTITY_2_ID: "Entity Two"
- Text: "Hello Universe"  
- Number: "100"
- Checkbox: "false"

TEST_ENTITY_3_ID: "Entity Three"
- Text: "Goodbye World"
- Number: "not-a-number" (invalid numeric)
```

### Relations
- Entity 1 → Entity 2 (TEST_RELATION_TYPE_ID)
- Entity 2 → Entity 3 (TEST_RELATION_TYPE_ID)

## Key Features

### Test Isolation
- Each test generates fresh UUIDs to avoid conflicts
- Helper function `filterToTestEntities()` isolates results to test data only
- Proper cleanup in `afterEach()` removes all test data

### Comprehensive Coverage
- Tests all filter types supported by the GraphQL API
- Includes edge cases and error conditions
- Verifies both positive and negative filter conditions

### Integration Testing
- Tests against real database with actual Drizzle ORM queries
- Uses the same Effect.js service layer as production code
- Validates complete request-to-response flow

## Known Issues

### Complex NOT Filter Limitation
The complex NOT filter (`NOT: { value: { ... } }`) currently has an implementation issue where it doesn't return entities as expected. The test documents this behavior:

```typescript
// This works correctly (property-level NOT)
value: { 
  property: "...", 
  text: { NOT: { contains: "Hello" } } 
}

// This has an issue (entity-level NOT)  
NOT: { 
  value: { 
    property: "...", 
    text: { contains: "Hello" } 
  } 
}
```

The issue is in the `buildEntityWhere` function's handling of top-level NOT conditions with EXISTS clauses.

## Configuration

### Database Connection
Tests use the same database configuration as the main application through the Effect.js Environment service. Ensure `DATABASE_URL` is set in your environment.

### Dependencies
- **vitest**: Test runner
- **Effect.js**: Service layer and dependency injection
- **Drizzle ORM**: Database queries
- **uuid**: Unique identifier generation

## Adding New Filter Tests

To add tests for new filter types:

1. **Add test data** in the `beforeEach()` setup
2. **Create test cases** following the existing pattern
3. **Use the helper function** `filterToTestEntities()` to isolate results
4. **Clean up** any additional data in `afterEach()`

### Example Test Pattern
```typescript
it("should filter by new condition", async () => {
  const filter: EntityFilter = {
    value: {
      property: PROPERTY_ID,
      newFilterType: {
        condition: "value",
      },
    },
  }

  const result = await Effect.runPromise(
    getEntities({filter}).pipe(provideDeps)
  )

  const testResults = filterToTestEntities(result)
  expect(testResults).toHaveLength(expectedCount)
  expect(testResults[0].id).toBe(EXPECTED_ENTITY_ID)
})
```

## Debugging

The test suite includes debug utilities:

- Console logging for complex filter results
- Helper tests that show actual vs expected behavior  
- Isolation verification to ensure test data integrity

Use the debug test to investigate filter behavior:
```bash
bun test src/__tests__/filters.test.ts --grep "debug complex NOT filter"
```