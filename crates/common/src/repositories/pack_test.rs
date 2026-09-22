//! Pack Test Repository
//!
//! Database operations for pack test execution tracking.

use crate::error::Result;
use crate::models::{Id, PackLatestTest, PackTestExecution, PackTestResult, PackTestStats};
use sqlx::{PgPool, Row};

/// Repository for pack test operations
pub struct PackTestRepository {
    pool: PgPool,
}

impl PackTestRepository {
    /// Create a new pack test repository
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a new pack test execution record
    pub async fn create(
        &self,
        pack_id: Id,
        pack_version: &str,
        trigger_reason: &str,
        result: &PackTestResult,
    ) -> Result<PackTestExecution> {
        let result_json = serde_json::to_value(result)?;

        let record = sqlx::query_as::<_, PackTestExecution>(
            r#"
            INSERT INTO pack_test_execution (
                pack_id, pack_version, execution_time, trigger_reason,
                total_tests, passed, failed, skipped, pass_rate, duration_ms, result
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING *
            "#,
        )
        .bind(pack_id)
        .bind(pack_version)
        .bind(result.execution_time)
        .bind(trigger_reason)
        .bind(result.total_tests)
        .bind(result.passed)
        .bind(result.failed)
        .bind(result.skipped)
        .bind(result.pass_rate)
        .bind(result.duration_ms)
        .bind(result_json)
        .fetch_one(&self.pool)
        .await?;

        Ok(record)
    }

    /// Find pack test execution by ID
    pub async fn find_by_id(&self, id: Id) -> Result<Option<PackTestExecution>> {
        let record = sqlx::query_as::<_, PackTestExecution>(
            r#"
            SELECT * FROM pack_test_execution
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
    }

    /// List all test executions for a pack
    pub async fn list_by_pack(
        &self,
        pack_id: Id,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<PackTestExecution>> {
        let records = sqlx::query_as::<_, PackTestExecution>(
            r#"
            SELECT * FROM pack_test_execution
            WHERE pack_id = $1
            ORDER BY execution_time DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(pack_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Get latest test execution for a pack
    pub async fn get_latest_by_pack(&self, pack_id: Id) -> Result<Option<PackTestExecution>> {
        let record = sqlx::query_as::<_, PackTestExecution>(
            r#"
            SELECT * FROM pack_test_execution
            WHERE pack_id = $1
            ORDER BY execution_time DESC
            LIMIT 1
            "#,
        )
        .bind(pack_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(record)
    }

    /// Get latest test for all packs
    pub async fn get_all_latest(&self) -> Result<Vec<PackLatestTest>> {
        let records = sqlx::query_as::<_, PackLatestTest>(
            r#"
            SELECT * FROM pack_latest_test
            ORDER BY test_time DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Get test statistics for a pack
    pub async fn get_stats(&self, pack_id: Id) -> Result<PackTestStats> {
        let row = sqlx::query(
            r#"
            SELECT * FROM get_pack_test_stats($1)
            "#,
        )
        .bind(pack_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(PackTestStats {
            total_executions: row.get("total_executions"),
            successful_executions: row.get("successful_executions"),
            failed_executions: row.get("failed_executions"),
            avg_pass_rate: row.get("avg_pass_rate"),
            avg_duration_ms: row.get("avg_duration_ms"),
            last_test_time: row.get("last_test_time"),
            last_test_passed: row.get("last_test_passed"),
        })
    }

    /// Check if pack has recent passing tests
    pub async fn has_passing_tests(&self, pack_id: Id, hours_ago: i32) -> Result<bool> {
        let row = sqlx::query(
            r#"
            SELECT pack_has_passing_tests($1, $2) as has_passing
            "#,
        )
        .bind(pack_id)
        .bind(hours_ago)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get("has_passing"))
    }

    /// Count test executions by pack
    pub async fn count_by_pack(&self, pack_id: Id) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*) as count FROM pack_test_execution
            WHERE pack_id = $1
            "#,
        )
        .bind(pack_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.get("count"))
    }

    /// List test executions by trigger reason
    pub async fn list_by_trigger_reason(
        &self,
        trigger_reason: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<PackTestExecution>> {
        let records = sqlx::query_as::<_, PackTestExecution>(
            r#"
            SELECT * FROM pack_test_execution
            WHERE trigger_reason = $1
            ORDER BY execution_time DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(trigger_reason)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Get failed test executions for a pack
    pub async fn get_failed_by_pack(
        &self,
        pack_id: Id,
        limit: i64,
    ) -> Result<Vec<PackTestExecution>> {
        let records = sqlx::query_as::<_, PackTestExecution>(
            r#"
            SELECT * FROM pack_test_execution
            WHERE pack_id = $1 AND failed > 0
            ORDER BY execution_time DESC
            LIMIT $2
            "#,
        )
        .bind(pack_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Delete old test executions (cleanup)
    pub async fn delete_old_executions(&self, days_old: i32) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM pack_test_execution
            WHERE execution_time < NOW() - ($1 || ' days')::INTERVAL
            "#,
        )
        .bind(days_old)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

// TODO: Update these tests to use the new repository API (static methods)
// These tests are currently disabled due to repository refactoring
#[cfg(test)]
#[allow(dead_code)]
mod tests {
    // Disabled - needs update for new repository API
    /*
    async fn setup() -> (PgPool, PackRepository, PackTestRepository) {
        let config = DatabaseConfig::from_env();
        let db = Database::new(&config)
            .await
            .expect("Failed to create database");
        let pool = db.pool().clone();
        let pack_repo = PackRepository::new(pool.clone());
        let test_repo = PackTestRepository::new(pool.clone());
        (pool, pack_repo, test_repo)
    }

    #[tokio::test]
    async fn test_create_test_execution() {
        let (_pool, pack_repo, test_repo) = setup().await;

        // Create a test pack
        let pack = pack_repo
            .create("test_pack", "Test Pack", "Test pack for testing", "1.0.0")
            .await
            .expect("Failed to create pack");

        // Create test result
        let test_result = PackTestResult {
            pack_ref: "test_pack".to_string(),
            pack_version: "1.0.0".to_string(),
            execution_time: Utc::now(),
            status: TestStatus::Passed,
            total_tests: 10,
            passed: 8,
            failed: 2,
            skipped: 0,
            pass_rate: 0.8,
            duration_ms: 5000,
            test_suites: vec![TestSuiteResult {
                name: "Test Suite 1".to_string(),
                runner_type: "shell".to_string(),
                total: 10,
                passed: 8,
                failed: 2,
                skipped: 0,
                duration_ms: 5000,
                test_cases: vec![
                    TestCaseResult {
                        name: "test_1".to_string(),
                        status: TestStatus::Passed,
                        duration_ms: 500,
                        error_message: None,
                        stdout: Some("Success".to_string()),
                        stderr: None,
                    },
                    TestCaseResult {
                        name: "test_2".to_string(),
                        status: TestStatus::Failed,
                        duration_ms: 300,
                        error_message: Some("Test failed".to_string()),
                        stdout: None,
                        stderr: Some("Error output".to_string()),
                    },
                ],
            }],
        };

        // Create test execution
        let execution = test_repo
            .create(pack.id, "1.0.0", "manual", &test_result)
            .await
            .expect("Failed to create test execution");

        assert_eq!(execution.pack_id, pack.id);
        assert_eq!(execution.total_tests, 10);
        assert_eq!(execution.passed, 8);
        assert_eq!(execution.failed, 2);
        assert_eq!(execution.pass_rate, 0.8);
    }

    #[tokio::test]
    async fn test_get_latest_by_pack() {
        let (_pool, pack_repo, test_repo) = setup().await;

        // Create a test pack
        let pack = pack_repo
            .create("test_pack_2", "Test Pack 2", "Test pack 2", "1.0.0")
            .await
            .expect("Failed to create pack");

        // Create multiple test executions
        for i in 1..=3 {
            let test_result = PackTestResult {
                pack_ref: "test_pack_2".to_string(),
                pack_version: "1.0.0".to_string(),
                execution_time: Utc::now(),
                total_tests: i,
                passed: i,
                failed: 0,
                skipped: 0,
                pass_rate: 1.0,
                duration_ms: 1000,
                test_suites: vec![],
            };

            test_repo
                .create(pack.id, "1.0.0", "manual", &test_result)
                .await
                .expect("Failed to create test execution");
        }

        // Get latest
        let latest = test_repo
            .get_latest_by_pack(pack.id)
            .await
            .expect("Failed to get latest")
            .expect("No latest found");

        assert_eq!(latest.total_tests, 3);
    }

    #[tokio::test]
    async fn test_get_stats() {
        let (_pool, pack_repo, test_repo) = setup().await;

        // Create a test pack
        let pack = pack_repo
            .create("test_pack_3", "Test Pack 3", "Test pack 3", "1.0.0")
            .await
            .expect("Failed to create pack");

        // Create test executions
        for _ in 1..=5 {
            let test_result = PackTestResult {
                pack_ref: "test_pack_3".to_string(),
                pack_version: "1.0.0".to_string(),
                execution_time: Utc::now(),
                total_tests: 10,
                passed: 10,
                failed: 0,
                skipped: 0,
                pass_rate: 1.0,
                duration_ms: 2000,
                test_suites: vec![],
            };

            test_repo
                .create(pack.id, "1.0.0", "manual", &test_result)
                .await
                .expect("Failed to create test execution");
        }

        // Get stats
        let stats = test_repo
            .get_stats(pack.id)
            .await
            .expect("Failed to get stats");

        assert_eq!(stats.total_executions, 5);
        assert_eq!(stats.successful_executions, 5);
        assert_eq!(stats.failed_executions, 0);
    }
    */
}
