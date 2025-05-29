# API Tests Documentation

This document describes the test suite for the Gaia API, specifically focusing on the entity filtering functionality.

## Test Structure

The test suite is organized into comprehensive test categories in a single test file:

### Entity Filter Tests (`filters-working.test.ts`)
Comprehensive tests covering all filter functionality and edge cases.

**Coverage:**
- **Text filters**: is, contains, startsWith, endsWith, exists, NOT
- **Number filters**: is, lessThan, lessThanOrEqual, greaterThan, greaterThanOrEqual, exists, NOT
- **Checkbox filters**: is, exists  
- **Point filters**: is, exists
- **Relation filters**: fromRelation, toRelation with typeId, fromEntityId, toEntityId, spaceId
- **Logical operators**: AND, OR, NOT (including nested combinations)
- **Complex combinations**: Mixed value and relation filters
- **Edge cases**: Empty strings, zero values, negative numbers, Unicode, special characters
- **SQL structure validation**: Drizzle SQL object structure and properties
- **Security**: SQL injection prevention through parameterization

## Running Tests

### Prerequisites
Ensure you have the required dependencies installed:

```bash
bun install
```

### Run All Tests
```bash
bun test
```

### Run Tests with UI
```bash
bun run test:ui
```

### Run Tests Once (CI mode)
```bash
bun run test:run
```

### Run Specific Test Files
```bash
# Run filter tests
bun test filters-working.test.ts

# Run with pattern matching
bun test --grep "Text Filters"
bun test --grep "Relation Filters"
```

### Run Tests with Coverage
```bash
bun test --coverage
```

## Test Scenarios

All test scenarios are validated in the comprehensive test suite. Here are some key examples:

### Text Filter Examples
```typescript
// Exact match
{ value: { property: 'name', text: { is: 'John Doe' } } }

// Contains substring  
{ value: { property: 'description', text: { contains: 'important' } } }

// Starts with prefix
{ value: { property: 'title', text: { startsWith: 'Project' } } }

// Ends with suffix
{ value: { property: 'filename', text: { endsWith: '.pdf' } } }

// Field exists
{ value: { property: 'optional_field', text: { exists: true } } }
```

### Relation Filter Examples  
```typescript
// Entity has outgoing relation
{ fromRelation: { typeId: 'manages', toEntityId: 'employee-123' } }

// Entity has incoming relation
{ toRelation: { typeId: 'reports-to', fromEntityId: 'manager-456' } }

// Multiple relation conditions
{ fromRelation: { typeId: 'member-of', spaceId: 'org-123', toEntityId: 'project-456' } }

// Empty relation filter (matches any relation)
{ fromRelation: {} }
```

### Complex Filter Examples
```typescript
// Deeply nested with all filter types
{
  AND: [
    {
      OR: [
        { value: { property: 'type', text: { is: 'admin' } } },
        { value: { property: 'type', text: { is: 'moderator' } } }
      ]
    },
    { value: { property: 'age', number: { greaterThan: 18 } } },
    { fromRelation: { typeId: 'member-of' } },
    { NOT: { toRelation: { typeId: 'blocked-by' } } }
  ]
}
```

## Testing Approach

The tests focus on functional validation rather than complex mocking:

- **Direct Function Testing**: Tests call `buildEntityWhere` directly with various filter inputs
- **SQL Object Validation**: Verifies that Drizzle SQL objects are generated correctly
- **Structure Verification**: Validates SQL object properties (queryChunks, decoder, shouldInlineParams)
- **Edge Case Coverage**: Tests boundary conditions, empty values, and special characters
- **Type Safety**: Ensures TypeScript interfaces work correctly with all filter combinations

## Security and Edge Case Testing

The test suite validates:

- **SQL Injection Prevention**: Special characters and malicious inputs are safely parameterized
- **Unicode Support**: Emoji and international characters are handled correctly  
- **Numeric Edge Cases**: Zero, negative, decimal, and very large numbers
- **Empty Values**: Empty strings and undefined values are processed safely
- **Type Coercion**: Proper handling of different data types (strings, numbers, booleans)

## Test Results

Current test status: ✅ **46 tests passing**

The comprehensive test suite covers:
- ✅ All text filter operators (6 tests)
- ✅ All number filter operators (4 tests)  
- ✅ All checkbox filter scenarios (3 tests)
- ✅ Point filter functionality (2 tests)
- ✅ Complete relation filter coverage (7 tests)
- ✅ All logical operators (5 tests)
- ✅ Complex nested combinations (3 tests)
- ✅ Edge cases and security (7 tests)
- ✅ SQL structure validation (3 tests)
- ✅ Filter type coverage (6 tests)

## Validated Filter Capabilities

The tests confirm that all filter types work correctly:

- **Text Filters**: All operators (is, contains, startsWith, endsWith, exists, NOT)
- **Number Filters**: All comparison operators and existence checks
- **Checkbox Filters**: Boolean value matching and existence  
- **Point Filters**: Coordinate matching and existence
- **Relation Filters**: All relation fields (typeId, fromEntityId, toEntityId, spaceId)
- **Logical Operators**: Complex nested AND, OR, NOT combinations
- **Mixed Filters**: Value filters combined with relation filters

## Contributing

When adding new filter types or modifying existing ones:

1. Add tests to `filters-working.test.ts` covering the new functionality
2. Include tests for edge cases and error conditions
3. Verify SQL object structure and properties are correct
4. Update this documentation with new examples
5. Ensure all existing tests continue to pass

## Debugging Tests

To debug failing tests:

1. Run tests in watch mode: `bun test --watch`
2. Use the UI for interactive debugging: `bun run test:ui`
3. Add `console.log` statements to inspect SQL object structure
4. Use specific test patterns: `bun test --grep "specific test name"`
5. Check that `buildEntityWhere` returns the expected SQL object type
6. Verify filter parameters are structured correctly according to TypeScript interfaces