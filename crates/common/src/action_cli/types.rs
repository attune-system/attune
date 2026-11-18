use serde::{Deserialize, Serialize};

/// One immutable token, ordered together with the parameter bindings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LiteralArgument {
    pub literal: String,
    pub order: i64,
}

/// Action-input annotation; values and their paths are never duplicated here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CliBinding {
    Positional {
        order: i64,
        #[serde(default)]
        encoding: Encoding,
        #[serde(default)]
        expand: bool,
    },
    Option {
        order: i64,
        name: String,
        #[serde(default)]
        style: OptionStyle,
        #[serde(default)]
        encoding: Encoding,
        #[serde(default)]
        expand: bool,
        #[serde(default)]
        missing: MissingPolicy,
    },
    Flag {
        order: i64,
        name: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_name"
        )]
        false_name: Option<String>,
        #[serde(default)]
        missing: MissingPolicy,
    },
}

fn optional_name<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    // Omission is supported by serde(default); explicit null is not a name.
    String::deserialize(deserializer).map(Some)
}

impl CliBinding {
    pub(super) fn order(&self) -> i64 {
        match self {
            Self::Positional { order, .. }
            | Self::Option { order, .. }
            | Self::Flag { order, .. } => *order,
        }
    }

    pub(crate) fn expands(&self) -> bool {
        matches!(
            self,
            Self::Positional { expand: true, .. } | Self::Option { expand: true, .. }
        )
    }

    pub(crate) fn encoding(&self) -> Encoding {
        match self {
            Self::Positional { encoding, .. } | Self::Option { encoding, .. } => *encoding,
            Self::Flag { .. } => Encoding::Scalar,
        }
    }

    pub(super) fn requires_value(&self) -> bool {
        matches!(
            self,
            Self::Positional { .. }
                | Self::Option {
                    missing: MissingPolicy::Error,
                    ..
                }
                | Self::Flag {
                    missing: MissingPolicy::Error,
                    ..
                }
        )
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    #[default]
    Scalar,
    Json,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OptionStyle {
    #[default]
    Separate,
    Equals,
    Attached,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissingPolicy {
    #[default]
    Omit,
    Error,
}

/// Future process integration must explicitly select the admitted input route.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InputDelivery {
    None,
    Stdin,
}

/// Operators may lower these limits, but cannot raise the format's ceilings.
#[derive(Debug, Clone, Copy)]
pub struct ArgumentLimits {
    pub max_arguments: usize,
    pub max_argument_bytes: usize,
    pub max_total_bytes: usize,
}

impl Default for ArgumentLimits {
    fn default() -> Self {
        Self {
            max_arguments: 1024,
            max_argument_bytes: 16_384,
            max_total_bytes: 65_536,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidBinding,
    DuplicateOrder,
    MissingValue,
    InvalidValue,
    SecretValue,
    NoSecureDelivery,
    LimitExceeded,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidBinding => "action.cli.invalid_binding",
            Self::DuplicateOrder => "action.cli.duplicate_order",
            Self::MissingValue => "action.cli.missing_value",
            Self::InvalidValue => "action.cli.invalid_value",
            Self::SecretValue => "action.cli.secret_value",
            Self::NoSecureDelivery => "action.cli.no_secure_delivery",
            Self::LimitExceeded => "action.cli.limit_exceeded",
        })
    }
}

/// Diagnostics contain metadata locations, never supplied values or argv.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Schema(#[from] crate::value_schema::SchemaError),
    #[error("{code} at {path}")]
    Argument { code: ErrorCode, path: String },
}

pub(super) fn failure(code: ErrorCode, path: impl Into<String>) -> Error {
    Error::Argument {
        code,
        path: path.into(),
    }
}

pub type Result<T> = std::result::Result<T, Error>;
