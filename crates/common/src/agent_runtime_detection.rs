//! Runtime auto-detection for injected Attune agent binaries.
//!
//! This module probes the local system directly for well-known interpreters,
//! without requiring database access.

use crate::child_process_environment::ChildProcessEnvironment;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, info};

/// A runtime interpreter discovered on the local system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedRuntime {
    /// Canonical runtime name (for example, "python" or "node").
    pub name: String,

    /// Absolute path to the interpreter binary.
    pub path: String,

    /// Version string if the version command succeeded.
    pub version: Option<String>,
}

impl fmt::Display for DetectedRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.version {
            Some(v) => write!(f, "{} ({}, v{})", self.name, self.path, v),
            None => write!(f, "{} ({})", self.name, self.path),
        }
    }
}

struct RuntimeCandidate {
    name: &'static str,
    binaries: &'static [&'static str],
    version_args: &'static [&'static str],
    version_parser: VersionParser,
}

enum VersionParser {
    SemverLike,
    JavaStyle,
}

fn candidates() -> Vec<RuntimeCandidate> {
    vec![
        RuntimeCandidate {
            name: "shell",
            binaries: &["bash", "sh"],
            version_args: &["--version"],
            version_parser: VersionParser::SemverLike,
        },
        RuntimeCandidate {
            name: "python",
            binaries: &["python3", "python"],
            version_args: &["--version"],
            version_parser: VersionParser::SemverLike,
        },
        RuntimeCandidate {
            name: "node",
            binaries: &["node", "nodejs"],
            version_args: &["--version"],
            version_parser: VersionParser::SemverLike,
        },
        RuntimeCandidate {
            name: "ruby",
            binaries: &["ruby"],
            version_args: &["--version"],
            version_parser: VersionParser::SemverLike,
        },
        RuntimeCandidate {
            name: "go",
            binaries: &["go"],
            version_args: &["version"],
            version_parser: VersionParser::SemverLike,
        },
        RuntimeCandidate {
            name: "java",
            binaries: &["java"],
            version_args: &["-version"],
            version_parser: VersionParser::JavaStyle,
        },
        RuntimeCandidate {
            name: "r",
            binaries: &["Rscript"],
            version_args: &["--version"],
            version_parser: VersionParser::SemverLike,
        },
        RuntimeCandidate {
            name: "perl",
            binaries: &["perl"],
            version_args: &["--version"],
            version_parser: VersionParser::SemverLike,
        },
    ]
}

/// Detect available runtimes by probing the local system.
pub async fn detect_runtimes(environment: &ChildProcessEnvironment) -> Vec<DetectedRuntime> {
    info!("Starting runtime auto-detection...");

    let mut detected = Vec::new();

    for candidate in candidates() {
        match detect_single_runtime(&candidate, environment).await {
            Some(runtime) => {
                info!("  ✓ Detected: {}", runtime);
                detected.push(runtime);
            }
            None => {
                debug!("  ✗ Not found: {}", candidate.name);
            }
        }
    }

    info!(
        "Runtime auto-detection complete: found {} runtime(s): [{}]",
        detected.len(),
        detected
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    detected
}

async fn detect_single_runtime(
    candidate: &RuntimeCandidate,
    environment: &ChildProcessEnvironment,
) -> Option<DetectedRuntime> {
    for binary in candidate.binaries {
        if let Some(path) = which_binary(binary, environment).await {
            let version = get_version(
                &path,
                candidate.version_args,
                &candidate.version_parser,
                environment,
            )
            .await;

            return Some(DetectedRuntime {
                name: candidate.name.to_string(),
                path,
                version,
            });
        }
    }

    None
}

async fn which_binary(binary: &str, environment: &ChildProcessEnvironment) -> Option<String> {
    if binary == "bash" || binary == "sh" {
        let absolute_path = format!("/bin/{}", binary);
        if std::path::Path::new(&absolute_path).exists() {
            return Some(absolute_path);
        }
    }

    if environment.uses_workload_sandbox() {
        // Inspect the operator PATH without running another host subprocess.
        return crate::runtime_lifecycle::resolve_executable(binary, environment)
            .ok()
            .map(|path| path.to_string_lossy().into_owned());
    }

    let mut command = Command::new("which");
    environment.apply(command.as_std_mut());
    match command.arg(binary).output().await {
        Ok(output) if output.status.success() => {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if path.is_empty() {
                None
            } else {
                Some(path)
            }
        }
        Ok(_) => None,
        Err(e) => {
            debug!("'which' command failed ({}), trying 'command -v'", e);
            let mut command = Command::new("sh");
            environment.apply(command.as_std_mut());
            match command
                .args(["-c", &format!("command -v {}", binary)])
                .output()
                .await
            {
                Ok(output) if output.status.success() => {
                    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if path.is_empty() {
                        None
                    } else {
                        Some(path)
                    }
                }
                _ => None,
            }
        }
    }
}

async fn get_version(
    binary_path: &str,
    version_args: &[&str],
    parser: &VersionParser,
    environment: &ChildProcessEnvironment,
) -> Option<String> {
    let mut command = Command::new(binary_path);
    environment.apply(command.as_std_mut());
    command.args(version_args);
    let mut command = if environment.uses_workload_sandbox() {
        let path = Path::new(binary_path);
        let identity = path.canonicalize().ok()?;
        command.env("HOME", "/tmp").env("TMPDIR", "/tmp");
        Command::from(
            environment
                .sandbox_staged_workload(command.into_std(), &[], &[], &[(path, &identity)])
                .inspect_err(|error| debug!(%error, "Failed to isolate runtime version detection"))
                .ok()?,
        )
    } else {
        command
    };
    let output =
        match tokio::time::timeout(Duration::from_secs(10), command.kill_on_drop(true).output())
            .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                debug!(%error, binary_path, "Failed to run runtime version command");
                return None;
            }
            Err(_) => {
                debug!(binary_path, "Runtime version command timed out");
                return None;
            }
        };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    match parser {
        VersionParser::SemverLike => parse_semver_like(&combined),
        VersionParser::JavaStyle => parse_java_version(&combined),
    }
}

fn parse_semver_like(output: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?:v|go)?(\d+\.\d+(?:\.\d+)?)").ok()?;
    re.captures(output)
        .and_then(|captures| captures.get(1).map(|m| m.as_str().to_string()))
}

fn parse_java_version(output: &str) -> Option<String> {
    let quoted_re = regex::Regex::new(r#"version\s+"([^"]+)""#).ok()?;
    if let Some(captures) = quoted_re.captures(output) {
        return captures.get(1).map(|m| m.as_str().to_string());
    }

    parse_semver_like(output)
}

pub fn format_as_env_value(runtimes: &[DetectedRuntime]) -> String {
    runtimes
        .iter()
        .map(|r| r.name.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

pub fn print_detection_report_for_env(env_var_name: &str, runtimes: &[DetectedRuntime]) {
    println!("=== Attune Agent Runtime Detection Report ===");
    println!();

    if runtimes.is_empty() {
        println!("No runtimes detected!");
        println!();
        println!("The agent could not find any supported interpreter binaries.");
        println!("Ensure at least one of the following is installed and on PATH:");
        println!("  - bash / sh       (shell scripts)");
        println!("  - python3 / python (Python scripts)");
        println!("  - node / nodejs    (Node.js scripts)");
        println!("  - ruby             (Ruby scripts)");
        println!("  - go               (Go programs)");
        println!("  - java             (Java programs)");
        println!("  - Rscript          (R scripts)");
        println!("  - perl             (Perl scripts)");
    } else {
        println!("Detected {} runtime(s):", runtimes.len());
        println!();
        for rt in runtimes {
            let version_str = rt.version.as_deref().unwrap_or("unknown version");
            println!("  ✓ {:<10} {} ({})", rt.name, rt.path, version_str);
        }
    }

    println!();
    println!("{}={}", env_var_name, format_as_env_value(runtimes));
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires Linux Bubblewrap and unprivileged namespaces"]
    async fn agent_detection_probes_use_the_requested_sandbox() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::TempDir::new().unwrap();
        let executable = root.path().join("node");
        let private_file = root.path().join("manager-fixture");
        std::fs::write(&private_file, "fixture").unwrap();
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\ntest ! -e '{}' || exit 1\nprintf 'v3.14.15\\n'\n",
                private_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let environment = ChildProcessEnvironment::from_parent(
            [("PATH".into(), root.path().as_os_str().to_owned())],
            &[],
        )
        .unwrap()
        .with_workload_sandbox(crate::workload_sandbox::WorkloadSandbox::Bubblewrap);
        let runtimes = detect_runtimes(&environment).await;
        assert!(
            runtimes
                .iter()
                .any(|runtime| runtime.name == "node"
                    && runtime.version.as_deref() == Some("3.14.15"))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn version_detection_preserves_named_opt_ins_without_inheriting_credentials() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::TempDir::new().unwrap();
        let executable = root.path().join("runtime");
        std::fs::write(&executable, "#!/bin/sh\ntest \"$LD_LIBRARY_PATH\" = /configured/library || exit 19\ntest -z \"${ATTUNE__SECURITY__JWT_SECRET+x}\" || exit 20\nprintf 'v3.14.15\\n'\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let parent = vec![
            ("LD_LIBRARY_PATH".into(), "/configured/library".into()),
            ("ATTUNE__SECURITY__JWT_SECRET".into(), "dummy-jwt".into()),
        ];
        let minimal = ChildProcessEnvironment::from_parent(parent.clone(), &[]).unwrap();
        let selected =
            ChildProcessEnvironment::from_parent(parent, &["LD_LIBRARY_PATH".into()]).unwrap();
        assert_eq!(
            get_version(
                executable.to_str().unwrap(),
                &[],
                &VersionParser::SemverLike,
                &minimal
            )
            .await,
            None
        );
        assert_eq!(
            get_version(
                executable.to_str().unwrap(),
                &[],
                &VersionParser::SemverLike,
                &selected
            )
            .await
            .as_deref(),
            Some("3.14.15")
        );
    }

    #[test]
    fn test_parse_semver_like_python() {
        assert_eq!(
            parse_semver_like("Python 3.12.1"),
            Some("3.12.1".to_string())
        );
    }

    #[test]
    fn test_parse_semver_like_node() {
        assert_eq!(parse_semver_like("v20.11.0"), Some("20.11.0".to_string()));
    }

    #[test]
    fn test_parse_semver_like_go() {
        assert_eq!(
            parse_semver_like("go version go1.22.0 linux/amd64"),
            Some("1.22.0".to_string())
        );
    }

    #[test]
    fn test_parse_java_version_openjdk() {
        assert_eq!(
            parse_java_version(r#"openjdk version "21.0.1" 2023-10-17"#),
            Some("21.0.1".to_string())
        );
    }

    #[test]
    fn test_format_as_env_value_multiple() {
        let runtimes = vec![
            DetectedRuntime {
                name: "shell".to_string(),
                path: "/bin/bash".to_string(),
                version: Some("5.2.15".to_string()),
            },
            DetectedRuntime {
                name: "python".to_string(),
                path: "/usr/bin/python3".to_string(),
                version: Some("3.12.1".to_string()),
            },
        ];

        assert_eq!(format_as_env_value(&runtimes), "shell,python");
    }
}
