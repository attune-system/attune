//! Explicit parent-variable selection for pack-controlled subprocesses.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::process::Command;

const BASE_VARIABLES: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "TZ",
    "TMPDIR",
    "LC_ALL",
    "LC_CTYPE",
    "LC_NUMERIC",
    "LC_TIME",
    "LC_COLLATE",
    "LC_MONETARY",
    "LC_MESSAGES",
    "LC_PAPER",
    "LC_NAME",
    "LC_ADDRESS",
    "LC_TELEPHONE",
    "LC_MEASUREMENT",
    "LC_IDENTIFICATION",
];

#[cfg(windows)]
const WINDOWS_BASE_VARIABLES: &[&str] = &[
    "SystemRoot",
    "SystemDrive",
    "WINDIR",
    "PATHEXT",
    "USERPROFILE",
    "TEMP",
    "TMP",
];

#[derive(Clone)]
pub struct ChildProcessEnvironment {
    values: BTreeMap<OsString, OsString>,
    runtime_recipe_digests: BTreeSet<String>,
    workload_sandbox: crate::workload_sandbox::WorkloadSandbox,
}

impl fmt::Debug for ChildProcessEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChildProcessEnvironment")
            .field("variable_names", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Default for ChildProcessEnvironment {
    fn default() -> Self {
        Self::capture(&[]).expect("the default child environment has no invalid passthrough names")
    }
}

impl ChildProcessEnvironment {
    pub fn capture(passthrough: &[String]) -> crate::Result<Self> {
        Self::from_parent(std::env::vars_os(), passthrough)
    }

    pub fn from_parent(
        parent: impl IntoIterator<Item = (OsString, OsString)>,
        passthrough: &[String],
    ) -> crate::Result<Self> {
        validate_passthrough_env(passthrough)?;
        let parent: Vec<_> = parent.into_iter().collect();
        let runtime_recipe_digests = parent
            .iter()
            .find(|(name, _)| name == "ATTUNE_RUNTIME_RECIPE_DIGESTS")
            .map(|(_, value)| {
                value
                    .to_str()
                    .ok_or_else(|| crate::Error::validation("Runtime recipe digests must be UTF-8"))
            })
            .transpose()?
            .unwrap_or("")
            .split(',')
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        for digest in &runtime_recipe_digests {
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(crate::Error::validation(
                    "Runtime recipe admission requires lowercase SHA-256 digests",
                ));
            }
        }
        let mut names: BTreeSet<&str> = BASE_VARIABLES.iter().copied().collect();
        names.extend(passthrough.iter().map(String::as_str));
        #[cfg(windows)]
        names.extend(WINDOWS_BASE_VARIABLES.iter().copied());
        let values = parent
            .into_iter()
            .filter(|(key, _)| {
                key.to_str().is_some_and(|key| {
                    #[cfg(windows)]
                    {
                        names.iter().any(|name| name.eq_ignore_ascii_case(key))
                    }
                    #[cfg(not(windows))]
                    {
                        names.contains(key)
                    }
                })
            })
            .collect();
        Ok(Self {
            values,
            runtime_recipe_digests,
            workload_sandbox: Default::default(),
        })
    }

    /// Clear inheritance before applying the selected baseline and explicit overlays.
    pub fn apply(&self, command: &mut Command) {
        command.env_clear().envs(&self.values);
    }

    /// Operator policy captured at startup, never inherited by workload children.
    pub fn admits_runtime_recipe(&self, digest: &str) -> bool {
        // Recipes currently use direct Unix processes. An admitted digest cannot
        // override an isolation policy requiring an isolated launch adapter.
        matches!(
            self.workload_sandbox,
            crate::workload_sandbox::WorkloadSandbox::Disabled
        ) && self.runtime_recipe_digests.contains(digest)
    }

    pub fn runtime_recipe_digests(&self) -> impl Iterator<Item = &str> {
        self.runtime_recipe_digests
            .iter()
            .map(String::as_str)
            .filter(|digest| self.admits_runtime_recipe(digest))
    }

    pub fn with_workload_sandbox(
        mut self,
        sandbox: crate::workload_sandbox::WorkloadSandbox,
    ) -> Self {
        self.workload_sandbox = sandbox;
        self
    }

    pub fn sandbox_workload(
        &self,
        command: Command,
        read_paths: &[&std::path::Path],
    ) -> std::io::Result<Command> {
        self.workload_sandbox.wrap(command, read_paths)
    }

    pub fn sandbox_workload_with_invocation_name(
        &self,
        command: Command,
        read_paths: &[&std::path::Path],
        invocation_name: Option<&str>,
    ) -> std::io::Result<Command> {
        self.workload_sandbox.wrap_with_invocation_name(
            command,
            read_paths,
            invocation_name.map(std::ffi::OsStr::new),
        )
    }

    /// Verification may expose only its selected executable, never its parent
    /// directory (which can contain sibling credentials or writable runtimes).
    pub fn sandbox_probe(&self, command: Command) -> std::io::Result<Command> {
        self.workload_sandbox.require_local_execution()?;
        let program = std::path::Path::new(command.get_program());
        let selected = if program.is_absolute() {
            program.to_path_buf()
        } else {
            self.get("PATH")
                .and_then(|path| {
                    std::env::split_paths(path)
                        .filter(|root| root.is_absolute())
                        .map(|root| root.join(program))
                        .find(|path| path.is_file())
                })
                .ok_or_else(|| {
                    std::io::Error::other("verification executable not found in selected PATH")
                })?
        };
        let system = ["/usr", "/bin", "/sbin", "/lib", "/lib64"]
            .iter()
            .any(|root| selected.starts_with(root));
        let paths = if system {
            Vec::new()
        } else {
            vec![selected.as_path()]
        };
        self.sandbox_workload(command, &paths)
    }

    pub fn sandbox_setup(
        &self,
        command: Command,
        pack: &std::path::Path,
        staging: &std::path::Path,
    ) -> std::io::Result<Command> {
        self.workload_sandbox
            .wrap_stage(command, &[pack], &[staging])
    }

    pub fn setup_path<'a>(
        &self,
        stage: &'a std::path::Path,
        destination: &'a std::path::Path,
    ) -> &'a std::path::Path {
        if matches!(
            self.workload_sandbox,
            crate::workload_sandbox::WorkloadSandbox::Bubblewrap
        ) {
            destination
        } else {
            stage
        }
    }

    pub fn sandbox_install(
        &self,
        command: Command,
        pack: &std::path::Path,
        stage: &std::path::Path,
        destination: &std::path::Path,
    ) -> std::io::Result<Command> {
        self.workload_sandbox
            .wrap_install(command, pack, stage, destination)
    }

    pub fn get(&self, name: &str) -> Option<&OsStr> {
        #[cfg(windows)]
        {
            self.values.iter().find_map(|(key, value)| {
                key.to_str()
                    .filter(|key| key.eq_ignore_ascii_case(name))
                    .map(|_| value.as_os_str())
            })
        }
        #[cfg(not(windows))]
        {
            self.values.get(OsStr::new(name)).map(OsString::as_os_str)
        }
    }
}

pub fn validate_passthrough_env(names: &[String]) -> crate::Result<()> {
    let mut seen = BTreeSet::new();
    for name in names {
        let mut bytes = name.bytes();
        let valid_start = bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_');
        if !valid_start || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
            return Err(crate::Error::validation(format!(
                "Invalid passthrough environment name '{name}'; use an exact variable name, not a pattern"
            )));
        }
        if name.to_ascii_uppercase().starts_with("ATTUNE_") {
            return Err(crate::Error::validation(format!(
                "Passthrough environment name '{name}' is reserved; Attune context must be supplied by the service"
            )));
        }
        let comparison = if cfg!(windows) {
            name.to_ascii_uppercase()
        } else {
            name.clone()
        };
        if !seen.insert(comparison) {
            return Err(crate::Error::validation(format!(
                "Duplicate passthrough environment name '{name}'"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent() -> Vec<(OsString, OsString)> {
        [
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/home/pack"),
            ("LANG", "C.UTF-8"),
            ("LC_ALL", "C"),
            ("LC_SECRET", "dummy-locale-secret"),
            ("ATTUNE__SECURITY__JWT_SECRET", "dummy-jwt-secret"),
            ("ATTUNE_API_TOKEN", "dummy-service-token"),
            ("AWS_SECRET_ACCESS_KEY", "dummy-storage-secret"),
            ("HTTP_PROXY", "dummy-proxy"),
            ("SSL_CERT_FILE", "/owned/ca.pem"),
            ("SERVICE_ONLY", "dummy-service-secret"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect()
    }

    #[test]
    fn default_selection_excludes_credentials_and_ambient_runtime_settings() {
        let environment = ChildProcessEnvironment::from_parent(parent(), &[]).unwrap();
        assert_eq!(environment.get("PATH"), Some(OsStr::new("/usr/bin:/bin")));
        assert_eq!(environment.get("LC_ALL"), Some(OsStr::new("C")));
        for name in [
            "ATTUNE__SECURITY__JWT_SECRET",
            "ATTUNE_API_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "HTTP_PROXY",
            "SSL_CERT_FILE",
            "SERVICE_ONLY",
            "LC_SECRET",
        ] {
            assert!(
                environment.get(name).is_none(),
                "Unexpected selected variable {name}"
            );
        }
        let debug = format!("{environment:?}");
        assert!(!debug.contains("/home/pack"));
        assert!(!debug.contains("dummy"));
    }

    #[test]
    fn passthrough_is_exact_named_and_instance_local() {
        let environment = ChildProcessEnvironment::from_parent(
            parent(),
            &["HTTP_PROXY".into(), "SSL_CERT_FILE".into()],
        )
        .unwrap();
        assert_eq!(
            environment.get("HTTP_PROXY"),
            Some(OsStr::new("dummy-proxy"))
        );
        assert_eq!(
            environment.get("SSL_CERT_FILE"),
            Some(OsStr::new("/owned/ca.pem"))
        );
        let other = ChildProcessEnvironment::from_parent(parent(), &[]).unwrap();
        assert!(other.get("HTTP_PROXY").is_none());
        assert!(environment.get("SERVICE_ONLY").is_none());
    }

    #[test]
    fn invalid_passthrough_configuration_is_rejected() {
        for names in [
            vec![""],
            vec!["HTTP_*"],
            vec!["HTTP_PROXY=foo"],
            vec!["ATTUNE_API_TOKEN"],
            vec!["ATTUNE__DATABASE__URL"],
            vec!["HTTP_PROXY", "HTTP_PROXY"],
        ] {
            assert!(validate_passthrough_env(
                &names.into_iter().map(str::to_string).collect::<Vec<_>>()
            )
            .is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn selected_non_utf8_values_are_preserved() {
        use std::os::unix::ffi::OsStringExt;
        let value = OsString::from_vec(vec![b'/', 0xff]);
        let environment =
            ChildProcessEnvironment::from_parent([(OsString::from("HOME"), value.clone())], &[])
                .unwrap();
        assert_eq!(environment.get("HOME"), Some(value.as_os_str()));
        let mut command = Command::new("/bin/sh");
        environment.apply(&mut command);
        assert_eq!(
            command
                .get_envs()
                .find(|(key, _)| *key == "HOME")
                .and_then(|(_, value)| value),
            Some(value.as_os_str())
        );
    }
}

#[cfg(test)]
mod native_workload_guard_tests {
    use super::*;
    use crate::workload_sandbox::WorkloadSandbox;
    #[test]
    fn native_effects_required_denies_every_local_adapter_before_lookup_or_launch() {
        let environment = ChildProcessEnvironment::from_parent(std::iter::empty(), &[])
            .unwrap()
            .with_workload_sandbox(WorkloadSandbox::NativeEffectsRequired);
        let missing = std::path::Path::new("/owned-native-guard-not-a-real-pack");
        fn selected_command() -> Command {
            let mut command = Command::new("owned-private-unavailable-program");
            command
                .arg("owned-private-argument")
                .env("OWNED_SELECTED", "owned-private-value");
            command
        }
        for result in [
            environment.sandbox_workload(selected_command(), &[missing]),
            environment.sandbox_workload_with_invocation_name(
                selected_command(),
                &[missing],
                Some("owned-static-alias"),
            ),
            environment.sandbox_probe(selected_command()),
            environment.sandbox_setup(selected_command(), missing, missing),
            environment.sandbox_install(selected_command(), missing, missing, missing),
            WorkloadSandbox::NativeEffectsRequired.wrap_stage(
                selected_command(),
                &[missing],
                &[missing],
            ),
            WorkloadSandbox::NativeEffectsRequired.wrap_workspace(
                selected_command(),
                &[missing],
                missing,
                missing,
            ),
        ] {
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("local workload unexpectedly permitted"),
            };
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert_eq!(
                error.to_string(),
                "local workload execution denied; native process effects are required"
            );
        }
        let stage = std::path::Path::new("/owned-private-unpublished-stage");
        let destination = std::path::Path::new("/owned-published-runtime");
        assert_eq!(environment.setup_path(stage, destination), stage);
        // The guard does not replace or weaken the independent env clearing.
        let mut command = selected_command();
        environment.apply(&mut command);
        assert_eq!(command.get_envs().count(), 0);
    }
}
