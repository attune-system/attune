//! Utility functions for Attune services
//!
//! This module provides common utility functions used across all services.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Pagination parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pagination {
    /// Page number (0-indexed)
    #[serde(default)]
    pub page: u32,

    /// Number of items per page
    #[serde(default = "default_page_size")]
    pub page_size: u32,
}

fn default_page_size() -> u32 {
    50
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            page: 0,
            page_size: default_page_size(),
        }
    }
}

impl Pagination {
    /// Calculate the offset for SQL queries
    pub fn offset(&self) -> u32 {
        self.page * self.page_size
    }

    /// Get the limit for SQL queries
    pub fn limit(&self) -> u32 {
        self.page_size
    }

    /// Validate pagination parameters
    pub fn validate(&self) -> crate::Result<()> {
        if self.page_size == 0 {
            return Err(crate::Error::validation("Page size must be greater than 0"));
        }

        if self.page_size > 1000 {
            return Err(crate::Error::validation("Page size must not exceed 1000"));
        }

        Ok(())
    }
}

/// Paginated response wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedResponse<T> {
    /// The page items
    pub items: Vec<T>,

    /// Pagination metadata
    pub pagination: PaginationMetadata,
}

/// Pagination metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginationMetadata {
    /// Current page number
    pub page: u32,

    /// Number of items per page
    pub page_size: u32,

    /// Total number of items
    pub total: u64,

    /// Total number of pages
    pub total_pages: u32,

    /// Whether there is a next page
    pub has_next: bool,

    /// Whether there is a previous page
    pub has_prev: bool,
}

impl PaginationMetadata {
    /// Create pagination metadata
    pub fn new(pagination: &Pagination, total: u64) -> Self {
        let total_pages = ((total as f64) / (pagination.page_size as f64)).ceil() as u32;
        let has_next = pagination.page + 1 < total_pages;
        let has_prev = pagination.page > 0;

        Self {
            page: pagination.page,
            page_size: pagination.page_size,
            total,
            total_pages,
            has_next,
            has_prev,
        }
    }
}

/// Convert Duration to human-readable string
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    }
}

/// Format timestamp relative to now (e.g., "2 hours ago")
pub fn format_relative_time(timestamp: DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(timestamp);

    if duration.num_seconds() < 0 {
        return "in the future".to_string();
    }

    let secs = duration.num_seconds();
    if secs < 60 {
        format!("{} seconds ago", secs)
    } else if secs < 3600 {
        let mins = secs / 60;
        if mins == 1 {
            "1 minute ago".to_string()
        } else {
            format!("{} minutes ago", mins)
        }
    } else if secs < 86400 {
        let hours = secs / 3600;
        if hours == 1 {
            "1 hour ago".to_string()
        } else {
            format!("{} hours ago", hours)
        }
    } else {
        let days = secs / 86400;
        if days == 1 {
            "1 day ago".to_string()
        } else {
            format!("{} days ago", days)
        }
    }
}

/// Sanitize a reference string (lowercase, replace spaces with hyphens)
pub fn sanitize_ref(input: &str) -> String {
    input
        .to_lowercase()
        .trim()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect()
}

/// Generate a unique identifier
pub fn generate_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Truncate a string to a maximum length
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Redact sensitive information from strings
pub fn redact_sensitive(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    let visible_chars = s.len().min(4);
    let redacted_chars = s.len().saturating_sub(visible_chars);

    if redacted_chars == 0 {
        return "*".repeat(s.len());
    }

    format!(
        "{}{}",
        "*".repeat(redacted_chars),
        &s[s.len() - visible_chars..]
    )
}

/// Creates directories recursively with shared-volume permissions (`0o2775`).
///
/// In multi-container Docker environments, services may run as different UIDs
/// (e.g., API as UID 1000, workers as root) but share volumes. This function
/// ensures that newly created directory components are:
/// - Group-writable (`rwxrwxr-x`)
/// - Setgid-flagged so child entries inherit the parent directory's group
///
/// The volume root must be owned by the shared group (GID 1000 / `attune`)
/// and all containers must include that GID via `group_add` in Docker Compose.
///
/// Only directories that don't already exist are chmod'd — existing ones are
/// left untouched.
#[cfg(unix)]
pub async fn create_shared_dir_all(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // Collect ancestor components that need to be created
    let mut to_create: Vec<&Path> = Vec::new();
    {
        let mut current = path;
        while !current.exists() {
            to_create.push(current);
            match current.parent() {
                Some(p) if p != current => current = p,
                _ => break,
            }
        }
    }

    // Create the full tree first
    tokio::fs::create_dir_all(path).await?;

    // Set setgid + rwxrwxr-x on each newly created component so child
    // directories inherit the parent's group automatically.
    for dir in &to_create {
        let perms = std::fs::Permissions::from_mode(0o2775);
        if let Err(e) = tokio::fs::set_permissions(dir, perms).await {
            tracing::warn!(
                "Failed to set 0o2775 on '{}': {} (cross-service reads may fail)",
                dir.display(),
                e
            );
        }
    }

    Ok(())
}

/// Synchronous variant of [`create_shared_dir_all`].
#[cfg(unix)]
pub fn create_shared_dir_all_sync(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut to_create: Vec<&Path> = Vec::new();
    {
        let mut current = path;
        while !current.exists() {
            to_create.push(current);
            match current.parent() {
                Some(p) if p != current => current = p,
                _ => break,
            }
        }
    }

    std::fs::create_dir_all(path)?;

    for dir in &to_create {
        let perms = std::fs::Permissions::from_mode(0o2775);
        if let Err(e) = std::fs::set_permissions(dir, perms) {
            tracing::warn!(
                "Failed to set 0o2775 on '{}': {} (cross-service reads may fail)",
                dir.display(),
                e
            );
        }
    }

    Ok(())
}

/// Windows variant: just creates directories, no setgid bits.
#[cfg(windows)]
pub fn create_shared_dir_all_sync(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Windows variant: just creates directories, no setgid bits.
#[cfg(windows)]
pub async fn create_shared_dir_all(path: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pagination_offset() {
        let page = Pagination {
            page: 0,
            page_size: 10,
        };
        assert_eq!(page.offset(), 0);
        assert_eq!(page.limit(), 10);

        let page = Pagination {
            page: 2,
            page_size: 25,
        };
        assert_eq!(page.offset(), 50);
        assert_eq!(page.limit(), 25);
    }

    #[test]
    fn test_pagination_validation() {
        let page = Pagination {
            page: 0,
            page_size: 0,
        };
        assert!(page.validate().is_err());

        let page = Pagination {
            page: 0,
            page_size: 2000,
        };
        assert!(page.validate().is_err());

        let page = Pagination {
            page: 0,
            page_size: 50,
        };
        assert!(page.validate().is_ok());
    }

    #[test]
    fn test_pagination_metadata() {
        let pagination = Pagination {
            page: 1,
            page_size: 10,
        };
        let metadata = PaginationMetadata::new(&pagination, 45);

        assert_eq!(metadata.page, 1);
        assert_eq!(metadata.page_size, 10);
        assert_eq!(metadata.total, 45);
        assert_eq!(metadata.total_pages, 5);
        assert!(metadata.has_next);
        assert!(metadata.has_prev);
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h 1m");
        assert_eq!(format_duration(Duration::from_secs(86400)), "1d 0h");
    }

    #[test]
    fn test_sanitize_ref() {
        assert_eq!(sanitize_ref("My Action"), "my-action");
        assert_eq!(sanitize_ref("  Test  "), "test");
        assert_eq!(sanitize_ref("UPPERCASE"), "uppercase");
    }

    #[test]
    fn test_generate_id() {
        let id1 = generate_id();
        let id2 = generate_id();
        assert_ne!(id1, id2);
        assert_eq!(id1.len(), 36); // UUID v4 format
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("this is a long string", 10), "this is...");
        assert_eq!(truncate("abc", 3), "abc");
        assert_eq!(truncate("abcd", 3), "...");
    }

    #[test]
    fn test_redact_sensitive() {
        assert_eq!(redact_sensitive(""), "");
        assert_eq!(redact_sensitive("abc"), "***");
        assert_eq!(redact_sensitive("password123"), "*******d123");
        assert_eq!(redact_sensitive("secret"), "**cret");
    }
}
