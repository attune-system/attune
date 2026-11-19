//! Pure planning for the fixed legacy inline guest. Code admission and the
//! immutable action/provider release proof remain the trusted producer's job.
use crate::{planning::*, runner_protocol::MAX_START_CONTEXT};
use attune_common::{
    models::runtime::{InlineExecutionStrategy, RuntimeExecutionConfig},
    workload_launch::{LaunchInvocation, LaunchPlatform, LaunchRoot, PackAccess, RUNNER_ROOT},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InlineActionPlan {
    pub interpreter: String,
    pub arguments: Vec<String>,
    pub strategy: InlineExecutionStrategy,
    pub extension: String,
    pub shell_helpers: bool,
    pub source: String,
    pub has_runtime: bool,
}

/// A literal definition, after the owning producer has proved its immutable
/// release/ad-hoc binding and restored the actual frozen input provenance.
/// This check does not itself authenticate a release or classify derived code.
#[derive(Clone, Copy)]
pub struct LiteralInlineSource<'a>(&'a str);
impl<'a> LiteralInlineSource<'a> {
    pub fn from_snapshot(
        source: &'a str,
        captured_entrypoint: &str,
        frozen_paths: Option<&[String]>,
    ) -> Result<Self> {
        if frozen_paths.is_none() || source != captured_entrypoint || source.contains('\0') {
            return Err(PlanningError::Configuration);
        }
        Ok(Self(source))
    }
    pub fn as_str(self) -> &'a str {
        self.0
    }
}

impl InlineActionPlan {
    pub fn new(
        config: &RuntimeExecutionConfig,
        source: LiteralInlineSource<'_>,
        has_runtime: bool,
    ) -> Result<Self> {
        // Reuse the pure prepared-runtime interpreter selection. This path is
        // never opened or materialized; inline source exists only in the guest.
        let interpreter = interpreted_argv(
            config,
            "actions/inline-source",
            has_runtime,
            PackAccess::ReadOnly,
        )?
        .into_iter()
        .next()
        .ok_or(PlanningError::Configuration)?;
        let temporary = config.inline_execution.strategy == InlineExecutionStrategy::TempFile;
        let extension = if temporary {
            let extension = config.inline_execution.extension.as_deref().unwrap_or("");
            if extension.is_empty() || extension.starts_with('.') {
                extension.to_owned()
            } else {
                format!(".{extension}")
            }
        } else {
            String::new()
        };
        let plan = Self {
            interpreter,
            // Direct intentionally ignores args, suffix and shell helpers.
            // TempFile preserves the raw declared args; legacy does not render
            // parameter/template values into interpreter arguments.
            arguments: if temporary {
                config.interpreter.args.clone()
            } else {
                Vec::new()
            },
            strategy: config.inline_execution.strategy,
            extension,
            shell_helpers: temporary && config.inline_execution.inject_shell_helpers,
            source: source.as_str().into(),
            has_runtime,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<()> {
        if self.interpreter.is_empty()
            || self.interpreter.contains('\0')
            || self.source.contains('\0')
            || self.arguments.iter().any(|arg| arg.contains('\0'))
            || self.extension.contains(['/', '\\', '\0'])
            || self.extension.chars().any(char::is_control)
            || (!self.extension.is_empty() && !self.extension.starts_with('.'))
            || (self.strategy == InlineExecutionStrategy::Direct
                && (self.shell_helpers || !self.arguments.is_empty() || !self.extension.is_empty()))
        {
            return Err(PlanningError::Configuration);
        }
        if serde_json::to_vec(self)
            .map_err(|_| PlanningError::Configuration)?
            .len()
            > MAX_START_CONTEXT
        {
            return Err(PlanningError::Bounds);
        }
        Ok(())
    }

    pub fn argv(&self, platform: LaunchPlatform) -> Result<Vec<String>> {
        self.validate()?;
        let json = serde_json::to_string(self).map_err(|_| PlanningError::Configuration)?;
        let mut argv = vec![
            format!("{RUNNER_ROOT}/linux-{}", platform.native_arch()),
            "run-inline-action".into(),
        ];
        let mut rest = json.as_str();
        while !rest.is_empty() {
            let mut end = rest.len().min(RUNTIME_RECIPE_CHUNK_BYTES);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            argv.push(rest[..end].into());
            rest = &rest[end..];
        }
        crate::runner_protocol::encode_start(&LaunchInvocation {
            argv: argv.clone(),
            env: Default::default(),
            working_root: LaunchRoot::Pack,
        })
        .map_err(|_| PlanningError::Bounds)?;
        Ok(argv)
    }

    pub fn decode(arguments: &[String]) -> Result<Self> {
        if arguments.is_empty()
            || arguments.len() > MAX_START_CONTEXT / RUNTIME_RECIPE_CHUNK_BYTES + 1
        {
            return Err(PlanningError::Bounds);
        }
        let mut json = String::new();
        for part in arguments {
            if part.is_empty()
                || part.len() > RUNTIME_RECIPE_CHUNK_BYTES
                || part.len() > MAX_START_CONTEXT - json.len()
            {
                return Err(PlanningError::Bounds);
            }
            json.push_str(part);
        }
        let plan: Self = serde_json::from_str(&json).map_err(|_| PlanningError::Configuration)?;
        plan.validate()?;
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RuntimeExecutionConfig {
        serde_json::from_value(serde_json::json!({"interpreter":{"binary":"python3","args":["-u","literal{pack_dir}"]},"inline_execution":{"strategy":"temp_file","extension":"py"}})).unwrap()
    }
    fn make_plan(
        config: &RuntimeExecutionConfig,
        source: &str,
        has_runtime: bool,
        paths: Option<&[String]>,
    ) -> Result<InlineActionPlan> {
        InlineActionPlan::new(
            config,
            LiteralInlineSource::from_snapshot(source, source, paths)?,
            has_runtime,
        )
    }
    #[test]
    fn direct_and_tempfile_keep_distinct_declared_semantics() {
        let mut config = config();
        config.inline_execution.inject_shell_helpers = true;
        let plan = make_plan(&config, "print(1)", false, Some(&[])).unwrap();
        assert_eq!(plan.arguments, ["-u", "literal{pack_dir}"]);
        assert_eq!(plan.extension, ".py");
        assert!(plan.shell_helpers);
        config.inline_execution.strategy = InlineExecutionStrategy::Direct;
        let plan = make_plan(&config, "print(1)", false, Some(&[])).unwrap();
        assert!(plan.arguments.is_empty() && plan.extension.is_empty() && !plan.shell_helpers);
    }
    #[test]
    fn missing_restore_secret_paths_and_unprepared_runtime_fail_closed() {
        let mut config = config();
        assert!(make_plan(&config, "public", false, None).is_err());
        assert!(LiteralInlineSource::from_snapshot(
            "secret-derived",
            "literal",
            Some(&["$.code".into()])
        )
        .is_err());
        let unrelated = make_plan(&config, "literal", false, Some(&["$.password".into()])).unwrap();
        assert_eq!(unrelated.source, "literal");
        config.environment = Some(serde_json::from_value(serde_json::json!({"env_type":"virtualenv","dir_name":"venv","interpreter_path":"{env_dir}/bin/python3","create_command":["prepare"]})).unwrap());
        assert!(make_plan(&config, "public", false, Some(&[])).is_err());
        let plan = make_plan(&config, "public", true, Some(&[])).unwrap();
        assert_eq!(plan.interpreter, "/opt/attune/runtime/bin/python3");
        assert!(plan.has_runtime);
    }
    #[test]
    fn large_unicode_source_is_bounded_and_round_trips_on_both_platforms() {
        let source = "🦀".repeat(24000);
        for platform in [LaunchPlatform::LinuxAmd64, LaunchPlatform::LinuxArm64] {
            let plan = make_plan(&config(), &source, false, Some(&[])).unwrap();
            let argv = plan.argv(platform).unwrap();
            assert!(argv[0].ends_with(platform.native_arch()));
            assert!(argv[2..]
                .iter()
                .all(|part| part.len() <= RUNTIME_RECIPE_CHUNK_BYTES));
            let decoded = InlineActionPlan::decode(&argv[2..]).unwrap();
            assert_eq!(decoded.source, source);
        }
        assert!(InlineActionPlan::decode(&["x".repeat(RUNTIME_RECIPE_CHUNK_BYTES + 1)]).is_err());
        assert!(make_plan(&config(), &"x".repeat(MAX_START_CONTEXT), false, Some(&[])).is_err());
    }
    #[test]
    fn malformed_suffix_or_recipe_is_not_silently_converted() {
        let mut config = config();
        config.inline_execution.extension = Some("../../escape".into());
        assert!(make_plan(&config, "public", false, Some(&[])).is_err());
        assert!(InlineActionPlan::decode(&["{}".into()]).is_err());
        assert!(make_plan(&config, "embedded\0source", false, Some(&[])).is_err());
        config.inline_execution.extension = Some("py".into());
        config.lifecycle = Some(serde_json::from_value(serde_json::json!({
            "tools":[{"name":"python","binaries":["/usr/bin/python3"],"args":["--version"],"version_pattern":"Python (?<version>[0-9.]+)","primary":true}],
            "launch":{"command":["{tool_python}","{source_file}"],"timeout_seconds":30}
        })).unwrap());
        assert!(matches!(
            make_plan(&config, "public", false, Some(&[])),
            Err(PlanningError::Lifecycle)
        ));
    }
}
