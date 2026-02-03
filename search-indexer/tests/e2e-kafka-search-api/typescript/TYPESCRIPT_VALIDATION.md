# TypeScript Search Validation

This directory includes a TypeScript-based validation script that tests the search API responses using the actual type definitions from the codebase.

## Overview

Instead of using bash/curl for API validation, we now use a type-safe TypeScript script that:

1. Imports actual types from `api/src/services/search/types.ts`
2. Constructs type-safe `SearchQuery` objects
3. Validates `SearchResponse` structures
4. Checks `SearchResult` fields and ordering

## Files

All TypeScript validation files are located in the `typescript/` subdirectory:

- **validate-search.ts** - Main validation script with 14 comprehensive tests
- **package.json** - Node.js dependencies (tsx for TypeScript execution)
- **tsconfig.json** - TypeScript configuration
- **TYPESCRIPT_VALIDATION.md** - This documentation file

## Usage

### Quick Test Script (Recommended)

The validation runs automatically when using run-test.sh from the parent directory:

```bash
cd ..
./run-test.sh
```

This will:
1. Generate test events
2. Check if search API is running
3. Install npm dependencies if needed
4. Run TypeScript validation tests

### Manual Execution

You can also run validation independently from the `typescript/` directory:

```bash
# From the typescript directory
cd typescript

# Install dependencies
npm install

# Run validation
npm run validate

# Or directly with tsx
npx tsx validate-search.ts
```

## Validation Tests

### Test 1: Basic Alice Search
- Query: "alice" with GLOBAL scope
- Expects: 7 entities all named "Alice"
- Validates: Score-based ordering (highest score first)
- Checks: First result is "Alice High" (high global score)

### Test 2: Bob Search
- Query: "bob" with GLOBAL scope
- Expects: 1 entity named "Bob"
- Validates: Description contains "project manager"

### Test 3: Organization Search
- Query: "acme" with GLOBAL scope
- Expects: 1 entity named "Acme Corp"

### Test 4: Entity Fields
- Validates presence of required fields:
  - `entityId` (string)
  - `name` (string)
  - `description` (string | null)
  - `typeIds` (array)
  - Scoring fields (entity_global_score, entity_space_score, space_score)

### Test 5: Score Ordering
- Validates entities are ordered by score (descending)
- Checks first entity has highest score
- Checks last entity has lowest score
- Validates entity_global_score descending order

### Test 6: Response Metadata
- Validates `total` count is present
- Validates `tookMs` execution time is present

### Test 7: Zero and Negative Scores
- Validates entities with zero (0.0) score are returned
- Validates entities with negative (-0.75) score are returned

### Test 8: TypeIds Scenarios
- Validates typeIds reflect type relation create/delete scenarios
- Alice High: Multiple types (Person + Organization)
- Alice Medium: Create->Delete->Create pattern works
- Alice Low: Partial type removal (Person kept after Org deleted)

### Test 9a: Deleted Entity Not In Results
- Verifies soft-deleted entity (Delete Charlie) is excluded from search
- Checks exclusion from both name search and broad search

### Test 9b: Restored Entity In Results
- Verifies restored entity (Delete Dana) appears in search results
- Dana was deleted then restored - should be visible
- Validates name and description are preserved after restore

### Test 10: Deleted Then Updated Entity
- Verifies entity deleted then updated (Delete Eve) remains excluded
- Post-delete updates should not resurrect the entity

### Test 11: Empty Query Top Ranked
- Validates empty query returns top-ranked results ordered by score
- Tests both GLOBAL and SPACE scopes

### Test 12: Unset Properties
- Validates unset_properties clears name/description while preserving other fields
- Test Case 1: Unset single property (name)
- Test Case 2: Unset multiple properties (name and description)

### Test 13: LWW Behavior
- Validates Last-Writer-Wins: sequential updates result in final value persisting
- Mixed set/unset operations on different properties

### Test 14: Include Deleted Flag
- Verifies `include_deleted` flag returns soft-deleted entities
- Charlie appears with flag set, data preserved
- Eve appears with flag set, tombstone dominance verified

## Type Safety Benefits

### SearchQuery Type
```typescript
interface SearchQuery {
  query: string;
  scope: SearchScope;
  space_id?: string;
  type_ids?: string[];
  limit?: number;
  offset?: number;
}
```

### SearchResponse Type
```typescript
interface SearchResponse {
  results: SearchResult[];
  total: number;
  tookMs: number;
}
```

### SearchResult Type
```typescript
interface SearchResult {
  entityId: string;
  spaceId: string;
  name: string;
  description: string | null;
  avatar: string | null;
  cover: string | null;
  typeIds: string[];
  entity_global_score?: number;
  entity_space_score?: number;
  space_score?: number;
}
```

## Benefits Over Bash/curl

1. **Type Safety**: Compile-time validation of query/response structures
2. **Maintainability**: Easier to update when API changes
3. **Code Reuse**: Uses same types as the API itself
4. **Better Errors**: TypeScript provides clear error messages
5. **IDE Support**: Autocomplete and inline documentation
6. **Testability**: Can be imported and used in integration tests

## Environment Variables

- `SEARCH_API_URL` - Override default API URL (default: http://localhost:3000)

Example:
```bash
SEARCH_API_URL=http://localhost:8080 npm run validate
```

## Exit Codes

- `0` - All tests passed
- `1` - One or more tests failed or API not available

## Integration with CI/CD

The validation script can be used in CI/CD pipelines:

```bash
#!/bin/bash
set -e

# Start services
docker-compose up -d

# Generate test data
./run-test.sh

# Validation runs automatically and exits with status code
# If validation fails, the script will exit with code 1
```

## Future Enhancements

Potential additions:

1. **Scope Testing**: Validate all SearchScope variants (GLOBAL_BY_SPACE_SCORE, SPACE_SINGLE, SPACE)
2. **Type Filtering**: Test type_ids parameter
3. **Pagination**: Test limit/offset parameters
4. **Performance**: Measure and validate response times
5. **Negative Cases**: Test invalid queries, missing parameters
6. **Snapshot Testing**: Compare responses against known-good snapshots
