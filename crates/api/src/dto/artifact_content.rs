//! Content selectors are assertions, not authorization or pagination cursors.
use serde::Deserialize;
use utoipa::IntoParams;

#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
pub struct ArtifactContentSelector {
    /// Expected immutable version ID. A deleted/recreated version with the same
    /// version number must not be substituted. Omit for number/latest selection.
    #[param(minimum = 1)]
    pub version_id: Option<i64>,
}
