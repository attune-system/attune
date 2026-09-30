use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const BUILD_GIT_SHA: &str = env!("ATTUNE_BUILD_GIT_SHA");
/// Identity compiled into this binary, never read from deployment-time environment variables.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct BuildInfo {
    /// Semantic version of the platform workspace.
    pub version: String,
    /// Full source commit SHA, or "unknown" when the build had no revision metadata.
    pub git_sha: String,
}

impl BuildInfo {
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            git_sha: BUILD_GIT_SHA.to_string(),
        }
    }
}
