//! Pure pack-index policy validation; deliberately not wired into API/config loading.
//!
//! Deserialize policy objects directly (not through a JSON `Value`) to retain duplicate
//! field detection. Create and patch share an omission-aware input, but normalization
//! is explicit. Import integration must still bound entire JSON/YAML documents and
//! reject duplicate keys outside this object, YAML tags, and other non-JSON values.
//! The Git matcher is only for development Git inputs, never archive versions.

use regex::{Regex, RegexBuilder};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

pub const DEFAULT_GIT_REF_PATTERN: &str = r"^[0-9]+\.[0-9]+\.[0-9]+$";
pub const DEFAULT_REFRESH_INTERVAL_SECONDS: u32 = 3600;
pub const DEFAULT_LAZY_REFRESH_MIN_INTERVAL_SECONDS: u32 = 30;
const MAX_INTERVAL_SECONDS: u64 = 604_800;
const MAX_PATTERN_BYTES: usize = 4096;
// Internal technical ceiling for regex's compiled representation, not a deployment
// setting or a refresh-queue budget. Applied to both standalone and anchored forms.
const REGEX_COMPILED_SIZE_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMode {
    Manual,
    Automatic,
    Lazy,
}

/// Presence is distinct from null, including for non-nullable policy fields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PolicyValue<T> {
    #[default]
    Omitted,
    Null,
    Value(T),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyField {
    UpdateMode,
    RefreshIntervalSeconds,
    LazyRefreshMinIntervalSeconds,
    AllowedGitRefPattern,
    AllowUnindexedGitRefs,
}

impl PolicyField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UpdateMode => "update_mode",
            Self::RefreshIntervalSeconds => "refresh_interval_seconds",
            Self::LazyRefreshMinIntervalSeconds => "lazy_refresh_min_interval_seconds",
            Self::AllowedGitRefPattern => "allowed_git_ref_pattern",
            Self::AllowUnindexedGitRefs => "allow_unindexed_git_refs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PolicyErrorKind {
    #[error("invalid policy document")]
    InvalidDocument,
    #[error("unknown policy field")]
    UnknownField,
    #[error("duplicate policy field")]
    DuplicateField,
    #[error("incorrect policy field type")]
    WrongType,
    #[error("null is not allowed for this policy field")]
    NullNotAllowed,
    #[error("unsupported update mode")]
    InvalidMode,
    #[error("interval must be between 1 and 604800 seconds")]
    IntervalOutOfRange,
    #[error("interval is not applicable to the resulting update mode")]
    IntervalNotApplicable,
    #[error("pattern must contain 1 to 4096 UTF-8 bytes")]
    PatternLength,
    #[error("invalid Git ref pattern syntax")]
    InvalidPattern,
    #[error("Git ref pattern exceeds compiled size limit")]
    PatternTooComplex,
    #[error("feature_not_available")]
    FeatureNotAvailable,
}

/// Contains only fixed categories and recognized field names, never submitted values
/// or regex/parser diagnostics. Unknown fields deliberately have no field name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{kind}")]
pub struct PolicyError {
    pub field: Option<PolicyField>,
    pub kind: PolicyErrorKind,
}

impl PolicyError {
    fn new(field: PolicyField, kind: PolicyErrorKind) -> Self {
        Self {
            field: Some(field),
            kind,
        }
    }

    fn document(kind: PolicyErrorKind) -> Self {
        Self { field: None, kind }
    }
}

/// The five policy fields only, not a complete index-management request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PolicyInput {
    pub update_mode: PolicyValue<UpdateMode>,
    pub refresh_interval_seconds: PolicyValue<u64>,
    pub lazy_refresh_min_interval_seconds: PolicyValue<u64>,
    pub allowed_git_ref_pattern: PolicyValue<String>,
    pub allow_unindexed_git_refs: PolicyValue<bool>,
}

impl PolicyInput {
    /// Direct JSON boundary preserving typed validation failures. Syntax errors are
    /// sanitized; caller-owned request byte/depth limits remain an integration task.
    pub fn from_json(json: &str) -> Result<Self, PolicyError> {
        let mut error = None;
        let mut deserializer = serde_json::Deserializer::from_str(json);
        let result = deserialize_input(&mut deserializer, &mut error)
            .and_then(|input| deserializer.end().map(|()| input));
        result.map_err(|_| error.unwrap_or(PolicyError::document(PolicyErrorKind::InvalidDocument)))
    }

    /// Apply create defaults and validate without any I/O.
    pub fn create(&self) -> Result<IndexPolicy, PolicyError> {
        IndexPolicy::default().patched(self)
    }
}

impl<'de> Deserialize<'de> for PolicyInput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut error = None;
        deserialize_input(deserializer, &mut error).map_err(|_| {
            serde::de::Error::custom(
                error.unwrap_or(PolicyError::document(PolicyErrorKind::InvalidDocument)),
            )
        })
    }
}

fn deserialize_input<'de, D: Deserializer<'de>>(
    deserializer: D,
    error: &mut Option<PolicyError>,
) -> Result<PolicyInput, D::Error> {
    struct InputVisitor<'a>(&'a mut Option<PolicyError>);

    impl<'de> Visitor<'de> for InputVisitor<'_> {
        type Value = PolicyInput;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a policy object")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut input = PolicyInput::default();
            while let Some(key) = map.next_key::<String>()? {
                let field = match key.as_str() {
                    "update_mode" => PolicyField::UpdateMode,
                    "refresh_interval_seconds" => PolicyField::RefreshIntervalSeconds,
                    "lazy_refresh_min_interval_seconds" => {
                        PolicyField::LazyRefreshMinIntervalSeconds
                    }
                    "allowed_git_ref_pattern" => PolicyField::AllowedGitRefPattern,
                    "allow_unindexed_git_refs" => PolicyField::AllowUnindexedGitRefs,
                    _ => {
                        let error = PolicyError::document(PolicyErrorKind::UnknownField);
                        *self.0 = Some(error);
                        return Err(serde::de::Error::custom(error));
                    }
                };
                let value = map.next_value::<serde_json::Value>().map_err(|_| {
                    let error = PolicyError::new(field, PolicyErrorKind::InvalidDocument);
                    *self.0 = Some(error);
                    serde::de::Error::custom(error)
                })?;
                let result = match field {
                    PolicyField::UpdateMode => {
                        set_value(&mut input.update_mode, value, field, |v| {
                            let mode = v.as_str().ok_or(PolicyErrorKind::WrongType)?;
                            match mode {
                                "manual" => Ok(UpdateMode::Manual),
                                "automatic" => Ok(UpdateMode::Automatic),
                                "lazy" => Ok(UpdateMode::Lazy),
                                _ => Err(PolicyErrorKind::InvalidMode),
                            }
                        })
                    }
                    PolicyField::RefreshIntervalSeconds => set_value(
                        &mut input.refresh_interval_seconds,
                        value,
                        field,
                        interval_value,
                    ),
                    PolicyField::LazyRefreshMinIntervalSeconds => set_value(
                        &mut input.lazy_refresh_min_interval_seconds,
                        value,
                        field,
                        interval_value,
                    ),
                    PolicyField::AllowedGitRefPattern => {
                        set_value(&mut input.allowed_git_ref_pattern, value, field, |v| {
                            v.as_str()
                                .map(str::to_owned)
                                .ok_or(PolicyErrorKind::WrongType)
                        })
                    }
                    PolicyField::AllowUnindexedGitRefs => {
                        set_value(&mut input.allow_unindexed_git_refs, value, field, |v| {
                            v.as_bool().ok_or(PolicyErrorKind::WrongType)
                        })
                    }
                };
                if let Err(error) = result {
                    *self.0 = Some(error);
                    return Err(serde::de::Error::custom(error));
                }
            }
            Ok(input)
        }
    }

    deserializer.deserialize_map(InputVisitor(error))
}

fn interval_value(value: serde_json::Value) -> Result<u64, PolicyErrorKind> {
    if let Some(value) = value.as_u64() {
        Ok(value)
    } else if value.as_i64().is_some() {
        Err(PolicyErrorKind::IntervalOutOfRange)
    } else {
        Err(PolicyErrorKind::WrongType)
    }
}

fn set_value<T>(
    target: &mut PolicyValue<T>,
    value: serde_json::Value,
    field: PolicyField,
    parse: impl FnOnce(serde_json::Value) -> Result<T, PolicyErrorKind>,
) -> Result<(), PolicyError> {
    if !matches!(target, PolicyValue::Omitted) {
        return Err(PolicyError::new(field, PolicyErrorKind::DuplicateField));
    }
    *target = if value.is_null() {
        PolicyValue::Null
    } else {
        PolicyValue::Value(parse(value).map_err(|kind| PolicyError::new(field, kind))?)
    };
    Ok(())
}

/// Validated, immutable policy. Construction and updates cannot bypass validation.
/// `patched` returns a new value, leaving the original unchanged even on failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexPolicy {
    update_mode: UpdateMode,
    refresh_interval_seconds: Option<u32>,
    lazy_refresh_min_interval_seconds: Option<u32>,
    allowed_git_ref_pattern: GitRefPattern,
    allow_unindexed_git_refs: bool,
}

impl Default for IndexPolicy {
    fn default() -> Self {
        Self {
            update_mode: UpdateMode::Manual,
            refresh_interval_seconds: None,
            lazy_refresh_min_interval_seconds: None,
            allowed_git_ref_pattern: GitRefPattern::compile(DEFAULT_GIT_REF_PATTERN)
                .expect("built-in Git ref pattern must be valid"),
            allow_unindexed_git_refs: false,
        }
    }
}

impl IndexPolicy {
    pub fn update_mode(&self) -> UpdateMode {
        self.update_mode
    }

    pub fn refresh_interval_seconds(&self) -> Option<u32> {
        self.refresh_interval_seconds
    }

    pub fn lazy_refresh_min_interval_seconds(&self) -> Option<u32> {
        self.lazy_refresh_min_interval_seconds
    }

    pub fn allowed_git_ref_pattern(&self) -> &str {
        &self.allowed_git_ref_pattern.source
    }

    pub fn allow_unindexed_git_refs(&self) -> bool {
        self.allow_unindexed_git_refs
    }

    /// Syntax allowlist only: not authorization to install an unindexed ref, and
    /// never a restriction on canonical archive release versions.
    pub fn matches_git_ref(&self, git_ref: &str) -> bool {
        self.allowed_git_ref_pattern.compiled.is_match(git_ref)
    }

    pub fn patched(&self, patch: &PolicyInput) -> Result<Self, PolicyError> {
        use PolicyField as F;
        let mode = non_null(&patch.update_mode, self.update_mode, F::UpdateMode)?;
        let refresh_interval_seconds = normalize_interval(
            &patch.refresh_interval_seconds,
            self.refresh_interval_seconds,
            mode == UpdateMode::Automatic,
            DEFAULT_REFRESH_INTERVAL_SECONDS,
            F::RefreshIntervalSeconds,
        )?;
        let lazy_refresh_min_interval_seconds = normalize_interval(
            &patch.lazy_refresh_min_interval_seconds,
            self.lazy_refresh_min_interval_seconds,
            mode == UpdateMode::Lazy,
            DEFAULT_LAZY_REFRESH_MIN_INTERVAL_SECONDS,
            F::LazyRefreshMinIntervalSeconds,
        )?;
        let allow_unindexed_git_refs = non_null(
            &patch.allow_unindexed_git_refs,
            self.allow_unindexed_git_refs,
            F::AllowUnindexedGitRefs,
        )?;
        if allow_unindexed_git_refs {
            return Err(PolicyError::new(
                F::AllowUnindexedGitRefs,
                PolicyErrorKind::FeatureNotAvailable,
            ));
        }
        let allowed_git_ref_pattern = match &patch.allowed_git_ref_pattern {
            PolicyValue::Omitted => self.allowed_git_ref_pattern.clone(),
            PolicyValue::Null => {
                return Err(PolicyError::new(
                    F::AllowedGitRefPattern,
                    PolicyErrorKind::NullNotAllowed,
                ))
            }
            PolicyValue::Value(pattern) => GitRefPattern::compile(pattern)?,
        };
        Ok(Self {
            update_mode: mode,
            refresh_interval_seconds,
            lazy_refresh_min_interval_seconds,
            allowed_git_ref_pattern,
            allow_unindexed_git_refs,
        })
    }
}

fn non_null<T: Copy>(
    value: &PolicyValue<T>,
    stored: T,
    field: PolicyField,
) -> Result<T, PolicyError> {
    match value {
        PolicyValue::Omitted => Ok(stored),
        PolicyValue::Null => Err(PolicyError::new(field, PolicyErrorKind::NullNotAllowed)),
        PolicyValue::Value(value) => Ok(*value),
    }
}

fn normalize_interval(
    value: &PolicyValue<u64>,
    stored: Option<u32>,
    active: bool,
    default: u32,
    field: PolicyField,
) -> Result<Option<u32>, PolicyError> {
    match value {
        PolicyValue::Omitted => Ok(if active {
            Some(stored.unwrap_or(default))
        } else {
            None
        }),
        PolicyValue::Null if !active => Ok(None),
        PolicyValue::Null => Err(PolicyError::new(field, PolicyErrorKind::NullNotAllowed)),
        PolicyValue::Value(_) if !active => Err(PolicyError::new(
            field,
            PolicyErrorKind::IntervalNotApplicable,
        )),
        PolicyValue::Value(value) if !(1..=MAX_INTERVAL_SECONDS).contains(value) => {
            Err(PolicyError::new(field, PolicyErrorKind::IntervalOutOfRange))
        }
        PolicyValue::Value(value) => Ok(Some(*value as u32)),
    }
}

#[derive(Debug, Clone)]
struct GitRefPattern {
    source: String,
    compiled: Regex,
}

impl GitRefPattern {
    fn compile(source: &str) -> Result<Self, PolicyError> {
        let error = |kind| PolicyError::new(PolicyField::AllowedGitRefPattern, kind);
        if source.is_empty() || source.len() > MAX_PATTERN_BYTES {
            return Err(error(PolicyErrorKind::PatternLength));
        }
        let compile = |pattern: &str| {
            RegexBuilder::new(pattern)
                .size_limit(REGEX_COMPILED_SIZE_LIMIT)
                .build()
        };
        let map_error = |err| {
            error(match err {
                regex::Error::CompiledTooBig(_) => PolicyErrorKind::PatternTooComplex,
                _ => PolicyErrorKind::InvalidPattern,
            })
        };
        // Validate independently so unbalanced groups cannot escape our boundaries.
        compile(source).map_err(map_error)?;
        let anchored = format!(r"\A(?:{source})\z");
        let compiled = match compile(&anchored) {
            // A trailing comment under (?x) may consume the closing wrapper. A
            // newline terminates it; only try this after standalone syntax passed.
            Err(regex::Error::Syntax(_)) => compile(&format!("\\A(?:{source}\n)\\z")),
            result => result,
        }
        .map_err(map_error)?;
        Ok(Self {
            source: source.to_owned(),
            compiled,
        })
    }
}

impl PartialEq for GitRefPattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Eq for GitRefPattern {}

impl Serialize for GitRefPattern {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.source)
    }
}

#[cfg(test)]
mod tests;
