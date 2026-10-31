//! Validated opaque browse positions, not authorization permits. Every page
//! reapplies current visibility. Concurrent writes/retention are not a snapshot.

use attune_common::{crypto, repositories::artifact::ArtifactSearchFilters};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    dto::artifact::ArtifactCursorPage,
    middleware::{ApiError, ApiResult},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Position {
    Artifact { created: DateTime<Utc>, id: i64 },
    Version { version: i32, id: i64 },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Token {
    binding: String,
    position: Position,
}

pub(crate) struct Cursor {
    binding: String,
    position: Option<Position>,
    encryption_key: String,
}

impl Cursor {
    pub(crate) fn artifacts(
        raw: Option<&str>,
        endpoint: &str,
        filters: &ArtifactSearchFilters,
        jwt_secret: Option<&str>,
    ) -> ApiResult<Self> {
        Self::decode(
            raw,
            json!({
                "endpoint": endpoint, "order": "created_desc_id_desc",
                "scope": filters.scope, "owner": filters.owner, "type": filters.r#type,
                "visibility": filters.visibility, "classification": filters.classification,
                "execution": filters.execution, "name": filters.name_contains,
            }),
            false,
            jwt_secret,
        )
    }

    pub(crate) fn versions(
        raw: Option<&str>,
        artifact: i64,
        jwt_secret: Option<&str>,
    ) -> ApiResult<Self> {
        Self::decode(
            raw,
            json!({
                "endpoint": "artifact_versions", "artifact": artifact,
                "order": "version_desc_id_desc",
            }),
            true,
            jwt_secret,
        )
    }

    fn decode(
        raw: Option<&str>,
        binding: serde_json::Value,
        versions: bool,
        jwt_secret: Option<&str>,
    ) -> ApiResult<Self> {
        let secret = jwt_secret
            .filter(|secret| !secret.is_empty())
            .ok_or_else(|| {
                ApiError::InternalServerError("Artifact cursor encryption is not configured".into())
            })?;
        // The shared helper derives the AES key. Separate this purpose from JWT
        // signing and every other encrypted application value on all replicas.
        let encryption_key = format!("attune:artifact-browse-cursor:v1\0{secret}");
        let binding = hex::encode(Sha256::digest(binding.to_string().as_bytes()));
        let position = if let Some(raw) = raw {
            if raw.is_empty() || raw.len() > 4096 {
                return Err(invalid());
            }
            let bytes = URL_SAFE_NO_PAD.decode(raw).map_err(|_| invalid())?;
            let plaintext =
                crypto::decrypt(&STANDARD.encode(bytes), &encryption_key).map_err(|_| invalid())?;
            let token: Token = serde_json::from_str(&plaintext).map_err(|_| invalid())?;
            if token.binding != binding {
                return Err(invalid());
            }
            match &token.position {
                Position::Artifact { id, .. } if !versions && *id > 0 => {}
                Position::Version { version, id } if versions && *version > 0 && *id > 0 => {}
                _ => return Err(invalid()),
            }
            Some(token.position)
        } else {
            None
        };
        Ok(Self {
            binding,
            position,
            encryption_key,
        })
    }

    pub(crate) fn artifact_after(&self) -> Option<(DateTime<Utc>, i64)> {
        match self.position.as_ref()? {
            Position::Artifact { created, id } => Some((*created, *id)),
            _ => None,
        }
    }

    pub(crate) fn version_after(&self) -> Option<(i32, i64)> {
        match self.position.as_ref()? {
            Position::Version { version, id } => Some((*version, *id)),
            _ => None,
        }
    }

    pub(crate) fn page<T>(
        &self,
        mut rows: Vec<T>,
        per_page: u32,
        position: impl Fn(&T) -> Position,
    ) -> ApiResult<ArtifactCursorPage<T>> {
        let has_more = rows.len() > per_page as usize;
        rows.truncate(per_page as usize);
        let after = rows.last().map(position);
        self.scanned_page(rows, has_more, after)
    }

    /// Recorded consumers may deny every candidate. Resume after the last
    /// inspected row even when no eligible items were returned.
    pub(crate) fn scanned_page<T>(
        &self,
        items: Vec<T>,
        has_more: bool,
        after: Option<Position>,
    ) -> ApiResult<ArtifactCursorPage<T>> {
        let next_cursor = if has_more {
            after
                .map(|position| {
                    let plaintext = serde_json::to_string(&Token {
                        binding: self.binding.clone(),
                        position,
                    })
                    .map_err(|_| encoding_failed())?;
                    let ciphertext = crypto::encrypt(&plaintext, &self.encryption_key)
                        .map_err(|_| encoding_failed())?;
                    let encrypted_bytes =
                        STANDARD.decode(ciphertext).map_err(|_| encoding_failed())?;
                    Ok::<_, ApiError>(URL_SAFE_NO_PAD.encode(encrypted_bytes))
                })
                .transpose()?
        } else {
            None
        };
        Ok(ArtifactCursorPage {
            items,
            next_cursor,
            has_more,
        })
    }
}

fn invalid() -> ApiError {
    ApiError::BadRequest("Invalid artifact cursor or cursor/filter mismatch".into())
}

fn encoding_failed() -> ApiError {
    ApiError::InternalServerError("Artifact cursor encoding failed".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    const SECRET: Option<&str> = Some("test-only-artifact-cursor-secret-value");

    #[test]
    fn cursor_is_bound_to_endpoint_scope_filters_and_order() {
        let filters = ArtifactSearchFilters::default();
        let cursor = Cursor::artifacts(None, "global", &filters, SECRET).unwrap();
        let page = cursor
            .page(vec![1, 2], 1, |_| Position::Artifact {
                created: Utc::now(),
                id: 9_000_000_000,
            })
            .unwrap();
        let raw = page.next_cursor.as_deref();
        assert!(Cursor::artifacts(raw, "global", &filters, SECRET).is_ok());
        assert!(Cursor::artifacts(raw, "execution", &filters, SECRET).is_err());
        let changed = ArtifactSearchFilters {
            execution: Some(42),
            ..filters
        };
        assert!(Cursor::artifacts(raw, "global", &changed, SECRET).is_err());
        assert!(Cursor::versions(raw, 42, SECRET).is_err());
    }

    #[test]
    fn malformed_and_nonpositive_positions_are_rejected() {
        for value in ["", "!", "e30"] {
            assert!(Cursor::versions(Some(value), 42, SECRET).is_err());
        }
        let cursor = Cursor::versions(None, 42, SECRET).unwrap();
        let page = cursor
            .scanned_page(
                Vec::<()>::new(),
                true,
                Some(Position::Version { version: 0, id: 42 }),
            )
            .unwrap();
        assert!(Cursor::versions(page.next_cursor.as_deref(), 42, SECRET).is_err());
    }

    #[test]
    fn denied_candidate_pages_can_resume_without_items_or_totals() {
        let cursor = Cursor::versions(None, 42, SECRET).unwrap();
        let page = cursor
            .scanned_page(
                Vec::<()>::new(),
                true,
                Some(Position::Version {
                    version: 4,
                    id: 9000,
                }),
            )
            .unwrap();
        assert!(page.items.is_empty());
        assert!(page.has_more);
        let next = Cursor::versions(page.next_cursor.as_deref(), 42, SECRET).unwrap();
        assert_eq!(next.version_after(), Some((4, 9000)));
        let bytes = URL_SAFE_NO_PAD
            .decode(page.next_cursor.as_deref().unwrap())
            .unwrap();
        assert!(serde_json::from_slice::<Token>(&bytes).is_err());
        assert!(Cursor::versions(page.next_cursor.as_deref(), 43, SECRET).is_err());
        assert!(Cursor::versions(page.next_cursor.as_deref(), 42, Some("another secret")).is_err());
        for index in [0, bytes.len() / 2, bytes.len() - 1] {
            let mut tampered = bytes.clone();
            tampered[index] ^= 1;
            let raw = URL_SAFE_NO_PAD.encode(tampered);
            assert!(Cursor::versions(Some(&raw), 42, SECRET).is_err());
        }
    }
}
