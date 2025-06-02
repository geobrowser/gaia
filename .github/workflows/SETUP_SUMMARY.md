# Single Workflow Integration Testing Setup

## Overview

This repository uses **one GitHub Actions workflow** for all integration testing needs. Simple, fast, and focused on testing entity filters against PostgreSQL 16.

## The Workflow

**Integration Tests** (`integration-tests.yml`)
- **Triggers**: Push/PR to main/develop (any `api/` changes)
- **Runtime**: ~10 minutes
- **Database**: PostgreSQL 16
- **Tests**: All integration tests including 32 filter tests
- **Features**: Linting + coverage + artifacts

## Database Setup

**PostgreSQL 16 Service:**
```yaml
services:
  postgres:
    image: postgres:16
    env:
      POSTGRES_USER: test_user
      POSTGRES_PASSWORD: test_password
      POSTGRES_DB: gaia_test
```

**Schema Setup:**
```yaml
- name: Setup database schema
  run: bun run db:setup
```

## Local Development

```bash
# Install prerequisites
curl -fsSL https://bun.sh/install | bash
brew install postgresql@16  # macOS

# Project setup
cd gaia/api
bun install
export DATABASE_URL="postgresql://user:pass@localhost:5432/test_db"

# Database setup
createdb test_db
bun run db:setup

# Run tests
bun run test:ci
```

## Commands

| Command | Purpose |
|---------|---------|
| `bun run test:integration` | Run all integration tests |
| `bun run test:filters` | Run filter tests only |
| `bun run test:ci` | Run tests with coverage |
| `bun run db:setup` | Setup database with validation |

## Manual Trigger

```bash
# GitHub CLI
gh workflow run integration-tests.yml

# GitHub Web UI
# Actions → Integration Tests → Run workflow
```

## Environment Variables

**Required:**
- `DATABASE_URL`: PostgreSQL 16 connection string

**Optional:**
- `NODE_ENV`: Environment (default: test)
- `CI`: CI flag (auto-set)

## Test Coverage

- **32 Filter Integration Tests**: Complete filter functionality
- **Text Filters**: is, contains, startsWith, endsWith, exists, NOT
- **Number Filters**: comparisons, exists, numeric validation
- **Complex Filters**: AND, OR, NOT combinations
- **Edge Cases**: Non-existent properties, invalid data
- **Checkbox/Point**: Boolean and coordinate testing

## File Structure

```
.github/workflows/
├── integration-tests.yml   # Single workflow (10 min)
├── ci.yml                  # Rust build (separate)
└── README.md              # Documentation

api/
├── src/__tests__/
│   ├── filters.test.ts     # 32 filter integration tests
│   └── README.md           # Test documentation
├── scripts/
│   └── setup-test-db.ts    # Database setup script
└── package.json            # Test scripts
```

## Workflow Steps

1. **Checkout code**
2. **Setup Bun** (latest version)
3. **Cache dependencies** (faster builds)
4. **Install dependencies** (`bun install --frozen-lockfile`)
5. **Wait for PostgreSQL** (health checks)
6. **Setup database schema** (`bun run db:setup`)
7. **Run linting** (Biome)
8. **Run integration tests** (`bun run test:ci`)
9. **Upload artifacts** (coverage, results)

## Benefits

- **⚡ Simple**: One workflow handles everything
- **🚀 Fast**: 10-minute feedback cycle
- **🔧 Easy**: Single PostgreSQL version
- **📊 Complete**: Comprehensive test coverage
- **🎯 Focused**: Integration testing only
- **🔍 Clear**: Easy debugging and logs

## Triggers

**Automatic:**
- Push to `main` or `develop`
- Pull request to `main` or `develop`
- Any changes in `api/` directory

**Manual:**
- GitHub Actions UI
- GitHub CLI

## Troubleshooting

```bash
# Database connectivity
pg_isready -h localhost -p 5432
psql $DATABASE_URL -c "SELECT version();"

# Reset database
dropdb test_db && createdb test_db
bun run db:setup

# Debug tests
bun test src/__tests__/filters.test.ts --reporter=verbose

# Check workflow logs
# GitHub → Actions → Select run → View logs
```

## Adding Tests

1. Create test files in `api/src/__tests__/`
2. Follow existing patterns for setup/cleanup
3. Use `filterToTestEntities()` for isolation
4. Tests automatically run in workflow

This single workflow provides everything you need: comprehensive integration testing with minimal complexity and fast feedback.