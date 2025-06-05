# Integration Tests

This directory contains integration tests for the indexer system.

## `integration.rs`

Comprehensive tests covering the core indexer functionality with a real PostgreSQL database.

### Test Categories

**Core Functionality**
- `main` - End-to-end test covering entities, values, relations, and properties with CRUD operations

**Property Management**
- `test_property_no_overwrite` - Verifies properties cannot be overwritten once created
- `test_property_squashing` - Tests handling of duplicate property operations in single edit

**Space Indexing**
- `test_space_indexing_personal` - Tests indexing of personal spaces
- `test_space_indexing_public` - Tests indexing of public spaces  
- `test_space_indexing_mixed` - Tests mixed personal and public spaces
- `test_space_indexing_empty` - Tests handling of empty space vectors
- `test_space_indexing_duplicate_dao_addresses` - Tests same DAO address for different space types
- `test_space_indexing_with_edits` - Tests spaces indexed alongside entity edits

**Data Validation**
- `test_validation_rejects_invalid_number` - Verifies invalid numbers are rejected
- `test_validation_rejects_invalid_checkbox` - Verifies invalid checkbox values are rejected
- `test_validation_rejects_invalid_time` - Verifies invalid time values are rejected
- `test_validation_rejects_invalid_point` - Verifies invalid point coordinates are rejected
- `test_validation_allows_valid_data_mixed_with_invalid` - Tests selective processing of valid/invalid data

### Prerequisites

- PostgreSQL database with `DATABASE_URL` environment variable set
- Database must have the latest migrations applied including the spaces table

### Key Behaviors Tested

- Data validation prevents invalid values from being stored
- Valid data processes correctly even when mixed with invalid data
- Space ID generation using `derive_space_id` with GEO network
- Conflict resolution with `ON CONFLICT DO NOTHING` semantics
- Property data type enforcement