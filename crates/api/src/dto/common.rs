//! Common DTO types used across all API endpoints

use serde::{Deserialize, Deserializer, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Pagination parameters for list endpoints
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct PaginationParams {
    /// Page number (1-based)
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    /// Number of items per page
    #[serde(default = "default_page_size")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub page_size: u32,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    50
}

impl PaginationParams {
    /// Get the SQL offset value
    pub fn offset(&self) -> u32 {
        (self.page.saturating_sub(1)) * self.page_size
    }

    /// Get the SQL limit value
    pub fn limit(&self) -> u32 {
        self.page_size.min(100) // Max 100 items per page
    }
}

impl Default for PaginationParams {
    fn default() -> Self {
        Self {
            page: default_page(),
            page_size: default_page_size(),
        }
    }
}

/// Pagination plus optional catalog text search.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct PaginationSearchParams {
    /// Page number (1-based)
    #[serde(default = "default_page")]
    #[param(example = 1, minimum = 1)]
    pub page: u32,

    /// Number of items per page
    #[serde(default = "default_page_size")]
    #[param(example = 50, minimum = 1, maximum = 100)]
    pub page_size: u32,

    /// Keyword query. Whitespace-separated tokens are AND-matched against
    /// the catalog item's discovery fields.
    #[param(example = "monitoring cpu")]
    pub q: Option<String>,
}

impl PaginationSearchParams {
    pub fn pagination(&self) -> PaginationParams {
        PaginationParams {
            page: self.page,
            page_size: self.page_size,
        }
    }
}

/// Paginated response wrapper
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PaginatedResponse<T> {
    /// The page items
    pub items: Vec<T>,

    /// Pagination metadata
    pub pagination: PaginationMeta,
}

/// Pagination metadata
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PaginationMeta {
    /// Current page number (1-based)
    #[schema(example = 1)]
    pub page: u32,

    /// Number of items per page
    #[schema(example = 50)]
    pub page_size: u32,

    /// Total number of items, when an exact count was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 150, nullable = true)]
    pub total_items: Option<u64>,

    /// Total number of pages, when an exact count was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = 3, nullable = true)]
    pub total_pages: Option<u32>,

    /// Whether a previous page exists.
    #[schema(example = false)]
    pub has_previous: bool,

    /// Whether a next page exists.
    #[schema(example = true)]
    pub has_next: bool,
}

impl PaginationMeta {
    /// Create pagination metadata with exact totals.
    pub fn new(page: u32, page_size: u32, total_items: u64) -> Self {
        let total_pages = if page_size > 0 {
            ((total_items as f64) / (page_size as f64)).ceil() as u32
        } else {
            0
        };

        Self {
            page,
            page_size,
            total_items: Some(total_items),
            total_pages: Some(total_pages),
            has_previous: page > 1,
            has_next: page < total_pages,
        }
    }

    /// Create pagination metadata without exact totals.
    pub fn without_totals(page: u32, page_size: u32, has_next: bool) -> Self {
        Self {
            page,
            page_size,
            total_items: None,
            total_pages: None,
            has_previous: page > 1,
            has_next,
        }
    }
}

impl<T> PaginatedResponse<T> {
    /// Create a new paginated response
    pub fn new(items: Vec<T>, params: &PaginationParams, total_items: u64) -> Self {
        Self {
            items,
            pagination: PaginationMeta::new(params.page, params.page_size, total_items),
        }
    }

    /// Create a paginated response without exact total counts.
    pub fn without_totals(items: Vec<T>, params: &PaginationParams, has_next: bool) -> Self {
        Self {
            items,
            pagination: PaginationMeta::without_totals(params.page, params.page_size, has_next),
        }
    }
}

/// Standard API response wrapper
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApiResponse<T> {
    /// Response data
    pub data: T,

    /// Optional message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl<T> ApiResponse<T> {
    /// Create a new API response
    pub fn new(data: T) -> Self {
        Self {
            data,
            message: None,
        }
    }

    /// Create an API response with a message
    pub fn with_message(data: T, message: impl Into<String>) -> Self {
        Self {
            data,
            message: Some(message.into()),
        }
    }
}

/// Success message response (for operations that don't return data)
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SuccessResponse {
    /// Success indicator
    #[schema(example = true)]
    pub success: bool,

    /// Message describing the operation
    #[schema(example = "Operation completed successfully")]
    pub message: String,
}

impl SuccessResponse {
    /// Create a success response
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
        }
    }
}

pub fn deserialize_double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::<T>::deserialize(deserializer)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    fn test_pagination_params_offset() {
        let params = PaginationParams {
            page: 1,
            page_size: 10,
        };
        assert_eq!(params.offset(), 0);

        let params = PaginationParams {
            page: 2,
            page_size: 10,
        };
        assert_eq!(params.offset(), 10);

        let params = PaginationParams {
            page: 3,
            page_size: 25,
        };
        assert_eq!(params.offset(), 50);
    }

    #[test]
    fn test_pagination_params_limit() {
        let params = PaginationParams {
            page: 1,
            page_size: 50,
        };
        assert_eq!(params.limit(), 50);

        // Should cap at 100
        let params = PaginationParams {
            page: 1,
            page_size: 200,
        };
        assert_eq!(params.limit(), 100);
    }

    #[test]
    fn test_pagination_meta() {
        let meta = PaginationMeta::new(1, 10, 45);
        assert_eq!(meta.page, 1);
        assert_eq!(meta.page_size, 10);
        assert_eq!(meta.total_items, Some(45));
        assert_eq!(meta.total_pages, Some(5));
        assert!(!meta.has_previous);
        assert!(meta.has_next);

        let meta = PaginationMeta::new(2, 20, 100);
        assert_eq!(meta.total_pages, Some(5));
        assert!(meta.has_previous);
        assert!(meta.has_next);
    }

    #[test]
    fn test_pagination_meta_without_totals() {
        let meta = PaginationMeta::without_totals(3, 50, true);
        assert_eq!(meta.page, 3);
        assert_eq!(meta.page_size, 50);
        assert_eq!(meta.total_items, None);
        assert_eq!(meta.total_pages, None);
        assert!(meta.has_previous);
        assert!(meta.has_next);
    }

    #[test]
    fn test_paginated_response() {
        let data = vec![1, 2, 3, 4, 5];
        let params = PaginationParams::default();
        let response = PaginatedResponse::new(data.clone(), &params, 100);

        assert_eq!(response.items, data);
        assert_eq!(response.pagination.total_items, Some(100));
        assert_eq!(response.pagination.page, 1);
    }

    #[test]
    fn test_paginated_response_without_totals() {
        let data = vec![1, 2, 3];
        let params = PaginationParams::default();
        let response = PaginatedResponse::without_totals(data.clone(), &params, false);

        assert_eq!(response.items, data);
        assert_eq!(response.pagination.total_items, None);
        assert_eq!(response.pagination.total_pages, None);
        assert!(!response.pagination.has_next);
    }

    #[derive(Debug, Deserialize)]
    struct DoubleOptionHolder {
        #[serde(default, deserialize_with = "deserialize_double_option")]
        value: Option<Option<Vec<String>>>,
    }

    #[test]
    fn deserialize_double_option_preserves_missing_null_and_value() {
        let missing: DoubleOptionHolder = serde_json::from_str("{}").unwrap();
        assert_eq!(missing.value, None);

        let null_value: DoubleOptionHolder = serde_json::from_str(r#"{"value":null}"#).unwrap();
        assert_eq!(null_value.value, Some(None));

        let set_value: DoubleOptionHolder = serde_json::from_str(r#"{"value":["a","b"]}"#).unwrap();
        assert_eq!(set_value.value, Some(Some(vec!["a".into(), "b".into()])));
    }
}
