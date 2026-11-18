use crate::models::{ParameterDelivery, ParameterFormat};
use crate::pack_format::LaunchSpec;
use crate::{Error, Result};

pub const WORKER_CAPABILITY: &str = "action_cli_arguments_v1";

/// CLI-enabled definitions support only no input or stdin JSON.
pub fn validate_cli_input_delivery(
    cli_enabled: bool,
    delivery: ParameterDelivery,
    format: ParameterFormat,
) -> Result<()> {
    if cli_enabled && (delivery == ParameterDelivery::File || format != ParameterFormat::Json) {
        return Err(Error::validation(
            "action.launch.invalid: CLI parameters require no input or stdin JSON",
        ));
    }
    Ok(())
}

impl LaunchSpec {
    pub fn action_arguments(&self) -> Result<&[super::LiteralArgument]> {
        match self {
            Self::Command { arguments, .. } | Self::Native { arguments, .. } => Ok(arguments),
            _ => Err(Error::validation(
                "action.launch.invalid: This action launch kind is not executable by the native runner",
            )),
        }
    }

    pub fn default_parameter_delivery(&self) -> ParameterDelivery {
        match self {
            Self::Command { .. } => ParameterDelivery::None,
            _ => ParameterDelivery::Stdin,
        }
    }

    pub fn validate_action_source(
        &self,
        entrypoint: Option<&str>,
        delivery: ParameterDelivery,
        format: ParameterFormat,
    ) -> Result<()> {
        self.action_arguments()?;
        if entrypoint.is_some() {
            return Err(Error::validation(
                "action.launch.invalid: Exactly one action execution source is required",
            ));
        }
        match self {
            Self::Command { program, .. } => {
                // Authoring accepts Linux paths even when the CLI runs on another OS.
                let absolute = program.starts_with('/');
                if program.is_empty()
                    || program.len() > super::ArgumentLimits::default().max_argument_bytes
                    || program.contains(['\0', '\\'])
                    || (!absolute && program.contains('/'))
                    || matches!(program.as_str(), "." | "..")
                {
                    return Err(Error::validation(
                        "action.launch.invalid: Command program must be a literal name or absolute path",
                    ));
                }
            }
            Self::Native { artifact, .. } => {
                if !(1..=128).contains(&artifact.len())
                    || !artifact.as_bytes()[0].is_ascii_lowercase()
                    || !artifact.bytes().all(|c| {
                        c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c)
                    })
                {
                    return Err(Error::validation(
                        "action.launch.invalid: Invalid native artifact identifier",
                    ));
                }
            }
            _ => unreachable!("action_arguments checked the launch kind"),
        }
        validate_cli_input_delivery(
            matches!(self, Self::Command { .. })
                || !self.action_arguments()?.is_empty()
                || delivery == ParameterDelivery::None,
            delivery,
            format,
        )
    }
}

/// Native command launches must never acquire an interpreter through a version.
pub fn validate_native_runtime(
    runtime: &crate::models::Runtime,
    versions: &[crate::models::RuntimeVersion],
) -> Result<()> {
    if crate::runtime_detection::normalize_runtime_name(&runtime.name) != "native"
        || crate::execution_env::has_runtime_process_configuration(&runtime.execution_config)
        || versions.iter().any(|version| {
            crate::execution_env::has_runtime_process_configuration(&version.execution_config)
        })
    {
        return Err(Error::validation(
            "action.launch.invalid: A command launch requires a direct native runtime",
        ));
    }
    Ok(())
}
