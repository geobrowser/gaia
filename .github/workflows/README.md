# GitHub Actions Integration Tests

This directory contains the CI/CD workflow for the Gaia API integration tests.

## Single Workflow Approach

We use **one workflow** that handles all integration testing needs:

### Integration Tests (`integration-tests.yml`)
- **Purpose**: Complete integration testing suite
- **Triggers**: Push/PR to main/develop branches affecting API code
- **Runtime**: ~10 minutes
- **Database**: PostgreSQL 16

**What it does:**
- Sets up PostgreSQL 16 service with health checks
- Installs dependencies with Bun (cached for speed)
- Sets up database schema using Drizzle Kit
- Runs code linting with Biome
- Executes all integration tests (including 32 filter tests)
- Generates test coverage and artifacts

## Local Development Setup

### Prerequisites
```bash
# Install Bun
curl -fsSL https://bun.sh/install | bash

# Install PostgreSQL 16
brew install postgresql@16        # macOS
# OR
sudo apt-get install postgresql-16 postgresql-client-16  # Ubuntu
```

### Environment Setup
```bash
# Navigate to API directory
cd gaia/api

# Install dependencies
bun install

# Set up environment
export DATABASE_URL="postgresql://user:password@localhost:5432/gaia_test"
export NODE_ENV=test
```

### Database Setup
```bash
# Create test database
createdb gaia_test

# Setup schema
bun run db:setup

# Verify setup
psql $DATABASE_URL -c "\dt"
```

## Running Tests Locally

```bash
# Run all integration tests
bun run test:integration

# Run with coverage
bun run test:ci

# Run filter tests specifically
bun run test:filters

# Run with watch mode
bun test src/__tests__/filters.test.ts --watch
```

## Manual Workflow Trigger

```bash
# Using GitHub CLI
gh workflow run integration-tests.yml

# Using GitHub Web UI
# Go to Actions tab → Integration Tests → Run workflow
```

## Environment Variables

### Required
- `DATABASE_URL`: PostgreSQL 16 connection string

### Optional
- `NODE_ENV`: Environment setting (default: test)
- `CI`: CI environment flag (auto-set by GitHub)

## Available Commands

| Command | Purpose |
|---------|---------|
| `bun run test:integration` | Run all integration tests |
| `bun run test:filters` | Run filter integration tests |
| `bun run test:ci` | Run tests with coverage |
| `bun run db:setup` | Setup database schema with validation |
| `bun run db:push` | Push schema changes |

## Triggers

The workflow runs automatically when:
- Push to `main` or `develop` branches
- Pull request to `main` or `develop` branches
- Any changes in the `api/` directory

## Database Configuration

The workflow uses PostgreSQL 16 with the following setup:

```yaml
services:
  postgres:
    image: postgres:16
    env:
      POSTGRES_USER: test_user
      POSTGRES_PASSWORD: test_password
      POSTGRES_DB: gaia_test
    options: >-
      --health-cmd pg_isready
      --health-interval 10s
      --health-timeout 5s
      --health-retries 5
    ports:
      - 5432:5432
```

## Test Coverage

- **32 Filter Integration Tests**: Complete filter functionality testing
- **Text Filters**: is, contains, startsWith, endsWith, exists, NOT
- **Number Filters**: comparisons, exists, numeric validation
- **Complex Filters**: AND, OR, NOT combinations
- **Edge Cases**: Non-existent properties, invalid data
- **Checkbox/Point Filters**: Boolean and coordinate testing

## Troubleshooting

### Database Connection Issues
```bash
# Check PostgreSQL is running
pg_isready -h localhost -p 5432

# Test connection
psql $DATABASE_URL -c "SELECT version();"
```

### Schema Issues
```bash
# Reset database
dropdb gaia_test && createdb gaia_test
bun run db:setup
```

### Test Failures
```bash
# Run with verbose output
bun test src/__tests__/filters.test.ts --reporter=verbose

# Check database state
psql $DATABASE_URL -c "SELECT * FROM entities LIMIT 5;"
```

### GitHub Actions Issues
- Check workflow logs in Actions tab
- Verify PostgreSQL service started properly
- Look for database connection timeouts
- Check if dependencies installed correctly

## Adding New Tests

To add new integration tests:

1. Create test files in `api/src/__tests__/`
2. Follow existing patterns for database setup/cleanup
3. Use the `filterToTestEntities()` helper for test isolation
4. Tests will automatically run in the integration workflow

### Example Test Structure
```typescript
describe("New Feature Tests", () => {
  beforeEach(async () => {
    // Seed test data with unique UUIDs
  })

  afterEach(async () => {
    // Clean up test data
  })

  it("should test new functionality", async () => {
    const result = await Effect.runPromise(
      newFunction(args).pipe(provideDeps)
    )
    expect(result).toEqual(expected)
  })
})
```

This single workflow approach provides comprehensive integration testing with minimal complexity and fast feedback loops.