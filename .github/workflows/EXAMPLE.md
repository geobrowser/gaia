# Example: Integration Test Workflow

## Quick Start

```bash
# 1. Make a change to your API code
echo "// Updated filter logic" >> api/src/resolvers/filters.ts

# 2. Commit and push
git add .
git commit -m "Update filter logic"
git push origin main

# 3. Watch the workflow run automatically
# GitHub → Actions → Integration Tests
```

## What Happens When You Push

```
✅ Checkout code
✅ Setup Bun runtime
✅ Cache dependencies (saves ~30 seconds)
✅ Install dependencies
✅ Start PostgreSQL 16 service
✅ Wait for database to be ready
✅ Setup database schema
✅ Run linting checks
✅ Run 32 filter integration tests
✅ Upload test results and coverage
```

## Expected Output

```
✓ Entity Filters Integration Tests > Text Filters > should filter by exact text match
✓ Entity Filters Integration Tests > Text Filters > should filter by text contains
✓ Entity Filters Integration Tests > Complex Filters > should handle NOT filters
...
✓ 32 tests passed in 45.2s
```

## Manual Trigger Example

```bash
# Trigger from command line
gh workflow run integration-tests.yml

# Check status
gh run list --workflow=integration-tests.yml
```

## Local Testing Before Push

```bash
# Test locally first
export DATABASE_URL="postgresql://user:pass@localhost:5432/test_db"
cd gaia/api
bun run db:setup
bun run test:ci
```

## Workflow File Location

```
gaia/.github/workflows/integration-tests.yml
```

## Runtime: ~10 minutes

The workflow completes in about 10 minutes and gives you comprehensive feedback on your entity filter integration tests.