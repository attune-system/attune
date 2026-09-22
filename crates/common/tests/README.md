# Attune Common Library - Integration Tests

This directory contains integration tests for the Attune common library, specifically testing the database repository layer and migrations.

## Overview

The test suite includes:

- **Migration Tests** (`migration_tests.rs`) - Verify database schema, migrations, and constraints
- **Repository Tests** - Comprehensive CRUD and transaction tests for each repository:
  - `pack_repository_tests.rs` - Pack repository operations
  - `action_repository_tests.rs` - Action repository operations
  - Additional repository tests for all other entities
- **Test Helpers** (`helpers.rs`) - Fixtures, utilities, and common test setup

## Prerequisites

Before running the tests, ensure you have:

1. **PostgreSQL** installed and running
2. **Test database** created and configured
3. **Environment variables** set (via `.env.test`)

### Setting Up the Test Database

```bash
# Create the test database
make db-test-create

# Run migrations on test database
make db-test-migrate

# Or do both at once
make db-test-setup
```

To reset the test database:

```bash
make db-test-reset
```

## Running Tests

### Run All Integration Tests

```bash
# Automatic setup and run
make test-integration

# Or manually
cargo test --test '*' -p attune-common -- --test-threads=4
```

### Run Specific Test Files

```bash
# Run only migration tests
cargo test --test migration_tests -p attune-common

# Run only pack repository tests
cargo test --test pack_repository_tests -p attune-common

# Run only action repository tests
cargo test --test action_repository_tests -p attune-common
```

### Run Specific Tests

```bash
# Run a single test by name
cargo test test_create_pack -p attune-common

# Run tests matching a pattern
cargo test test_create -p attune-common

# Run with output
cargo test test_create_pack -p attune-common -- --nocapture
```

## Test Configuration

Test configuration is loaded from `.env.test` in the project root. Key settings:

```bash
# Test database URL
ATTUNE__DATABASE__URL=postgresql://postgres:postgres@localhost:5432/attune_test

# Enable SQL logging for debugging
ATTUNE__DATABASE__LOG_STATEMENTS=true

# Verbose logging
ATTUNE__LOG__LEVEL=debug
RUST_LOG=debug,sqlx=warn
```

## Test Structure

### Test Helpers (`helpers.rs`)

The helpers module provides:

- **Database Setup**: `create_test_pool()`, `clean_database()`
- **Fixtures**: Builder pattern for creating test data
  - `PackFixture` - Create test packs
  - `ActionFixture` - Create test actions
  - `RuntimeFixture` - Create test runtimes
  - And more for all entities
- **Utilities**: Transaction helpers, assertions

Example fixture usage:

```rust
use helpers::*;

let pool = create_test_pool().await.unwrap();
clean_database(&pool).await.unwrap();

let pack_repo = PackRepository::new(&pool);

// Use fixture to create test data
let pack = PackFixture::new("test.pack")
    .with_version("2.0.0")
    .with_name("Custom Pack Name")
    .create(&pack_repo)
    .await
    .unwrap();
```

### Test Organization

Each test file follows this pattern:

1. **Import helpers module**: `mod helpers;`
2. **Setup phase**: Create pool and clean database
3. **Test execution**: Perform operations
4. **Assertions**: Verify expected outcomes
5. **Cleanup**: Automatic via `clean_database()` or transactions

Example test:

```rust
#[tokio::test]
async fn test_create_pack() {
    // Setup
    let pool = create_test_pool().await.unwrap();
    clean_database(&pool).await.unwrap();
    
    let repo = PackRepository::new(&pool);
    
    // Execute
    let pack = PackFixture::new("test.pack")
        .create(&repo)
        .await
        .unwrap();
    
    // Assert
    assert_eq!(pack.ref_name, "test.pack");
    assert!(pack.created_at.timestamp() > 0);
}
```

## Test Categories

### CRUD Operations

Tests verify basic Create, Read, Update, Delete operations:

- Creating entities with valid data
- Retrieving entities by ID and other fields
- Listing and pagination
- Updating partial and full records
- Deleting entities

### Constraint Validation

Tests verify database constraints:

- Unique constraints (e.g., pack ref_name + version)
- Foreign key constraints
- NOT NULL constraints
- Check constraints

### Transaction Support

Tests verify transaction behavior:

- Commit preserves changes
- Rollback discards changes
- Isolation between transactions

### Error Handling

Tests verify proper error handling:

- Duplicate key violations
- Foreign key violations
- Not found scenarios

### Cascading Deletes

Tests verify cascade delete behavior:

- Deleting a pack deletes associated actions
- Deleting a runtime deletes associated workers
- And other cascade relationships

## Best Practices

### 1. Clean Database Before Tests

Always clean the database at the start of each test:

```rust
let pool = create_test_pool().await.unwrap();
clean_database(&pool).await.unwrap();
```

### 2. Use Fixtures for Test Data

Use fixture builders instead of manual creation:

```rust
// Good
let pack = PackFixture::new("test.pack").create(&repo).await.unwrap();

// Avoid
let create = CreatePack { /* ... */ };
let pack = repo.create(&create).await.unwrap();
```

### 3. Test Isolation

Each test should be independent:

- Don't rely on data from other tests
- Clean database between tests
- Use unique names/IDs

### 4. Single-Threaded Execution

Run integration tests single-threaded to avoid race conditions:

```bash
cargo test -- --test-threads=4
```

### 5. Descriptive Test Names

Use clear, descriptive test names:

```rust
#[tokio::test]
async fn test_create_pack_duplicate_ref_version() { /* ... */ }
```

### 6. Test Both Success and Failure

Test both happy paths and error cases:

```rust
#[tokio::test]
async fn test_create_pack() { /* success case */ }

#[tokio::test]
async fn test_create_pack_duplicate_ref_version() { /* error case */ }
```

## Debugging Tests

### Enable SQL Logging

Set in `.env.test`:

```bash
ATTUNE__DATABASE__LOG_STATEMENTS=true
RUST_LOG=debug,sqlx=debug
```

### Run with Output

```bash
cargo test test_name -- --nocapture
```

### Use Transaction Rollback

Wrap tests in transactions that rollback to inspect state:

```rust
let mut tx = pool.begin().await.unwrap();
// ... test operations ...
// Drop tx without commit to rollback
```

### Check Database State

Connect to test database directly:

```bash
psql -d attune_test -U postgres
```

## Continuous Integration

For CI environments:

```bash
# Setup test database
createdb attune_test
DATABASE_URL=postgresql://postgres:postgres@localhost:5432/attune_test sqlx migrate run

# Run tests
cargo test --test '*' -p attune-common -- --test-threads=4
```

## Common Issues

### Database Connection Errors

**Issue**: Cannot connect to database

**Solution**: 
- Ensure PostgreSQL is running
- Check credentials in `.env.test`
- Verify test database exists

### Migration Errors

**Issue**: Migrations fail

**Solution**:
- Run `make db-test-reset` to reset test database
- Ensure migrations are in `migrations/` directory

### Flaky Tests

**Issue**: Tests fail intermittently

**Solution**:
- Keep the database-test minimum: `--test-threads=4`
- Clean database before each test
- Avoid time-dependent assertions

### Foreign Key Violations

**Issue**: Cannot delete entity due to foreign keys

**Solution**:
- Use `clean_database()` which handles dependencies
- Test cascade deletes explicitly
- Delete in correct order (children before parents)

## Adding New Tests

To add tests for a new repository:

1. Create test file: `tests/<entity>_repository_tests.rs`
2. Import helpers: `mod helpers;`
3. Create fixtures in `helpers.rs` if needed
4. Write comprehensive CRUD tests
5. Test constraints and error cases
6. Test transactions
7. Run and verify: `cargo test --test <entity>_repository_tests`

## Test Coverage

To generate test coverage reports:

```bash
# Install tarpaulin
cargo install cargo-tarpaulin

# Generate coverage
cargo tarpaulin --out Html --output-dir coverage --test '*' -p attune-common
```

## Additional Resources

- [SQLx Documentation](https://docs.rs/sqlx)
- [Tokio Testing Guide](https://tokio.rs/tokio/topics/testing)
- [Rust Testing Best Practices](https://doc.rust-lang.org/book/ch11-00-testing.html)

## Support

For issues or questions:

- Check existing tests for examples
- Review helper functions in `helpers.rs`
- Consult the main project documentation
- Open an issue on the project repository
