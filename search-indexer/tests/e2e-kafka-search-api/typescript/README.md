# TypeScript Validation for Search API

Type-safe validation tests for the search API using actual TypeScript types from the codebase.

## Quick Start

```bash
# Install dependencies
npm install

# Run validation tests
npm run validate
```

## What This Does

This validation script:
- Imports actual types from `api/src/services/search/types.ts`
- Runs 6 comprehensive validation tests
- Validates entity counts, ordering, fields, and scores
- Provides color-coded test results
- Exits with proper status codes for CI/CD

## Files

- **validate-search.ts** - Main validation script (272 lines)
- **package.json** - Node.js dependencies
- **tsconfig.json** - TypeScript configuration
- **TYPESCRIPT_VALIDATION.md** - Detailed documentation

## Validation Tests

1. **Basic Alice Search** - Validates 7 Alice entities ordered by score
2. **Bob Search** - Validates single Bob entity
3. **Organization Search** - Validates Acme Corp entity
4. **Entity Fields** - Checks required fields (entityId, name, description, typeIds, scores)
5. **Score Ordering** - Validates descending score order
6. **Response Metadata** - Checks total count and execution time

## Environment Variables

- `SEARCH_API_URL` - API URL (default: http://localhost:3000)

Example:
```bash
SEARCH_API_URL=http://localhost:8080 npm run validate
```

## Integration

This validation is automatically run by `../run-test.sh` when the search API is detected.

## See Also

- [TYPESCRIPT_VALIDATION.md](./TYPESCRIPT_VALIDATION.md) - Complete documentation
- [../README.md](../README.md) - Main e2e-kafka-search-api documentation
