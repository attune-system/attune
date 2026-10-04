//! Shared bootstrap helpers for injected agent binaries.

use crate::agent_runtime_detection::{
    detect_runtimes, format_as_env_value, print_detection_report_for_env, DetectedRuntime,
};
use crate::child_process_environment::ChildProcessEnvironment;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct RuntimeBootstrapResult {
    pub runtimes_override: Option<String>,
    pub detected_runtimes: Option<Vec<DetectedRuntime>>,
}

/// Detect runtimes and populate the agent runtime environment variable when needed.
///
/// This must run before the Tokio runtime starts because it may mutate process
/// environment variables.
pub fn bootstrap_runtime_env(
    env_var_name: &str,
    environment: &ChildProcessEnvironment,
) -> RuntimeBootstrapResult {
    let runtimes_override = std::env::var(env_var_name).ok();
    let mut detected_runtimes = None;

    if let Some(ref override_value) = runtimes_override {
        info!(
            "{} already set (override): {}",
            env_var_name, override_value
        );
        info!("Running auto-detection for override-specified runtimes...");

        let detected = detect_runtimes(environment);
        let override_names: Vec<&str> = override_value.split(',').map(|s| s.trim()).collect();

        let filtered: Vec<_> = detected
            .into_iter()
            .filter(|rt| {
                let lower_name = rt.name.to_ascii_lowercase();
                override_names
                    .iter()
                    .any(|ov| ov.to_ascii_lowercase() == lower_name)
            })
            .collect();

        if filtered.is_empty() {
            warn!(
                "None of the override runtimes ({}) were found on this system",
                override_value
            );
        } else {
            info!(
                "Matched {} override runtime(s) to detected interpreters:",
                filtered.len()
            );
            for rt in &filtered {
                match &rt.version {
                    Some(ver) => info!("  ✓ {} — {} ({})", rt.name, rt.path, ver),
                    None => info!("  ✓ {} — {}", rt.name, rt.path),
                }
            }
            detected_runtimes = Some(filtered);
        }
    } else {
        info!("No {} override — running auto-detection...", env_var_name);

        let detected = detect_runtimes(environment);

        if detected.is_empty() {
            warn!("No runtimes detected! The agent may not be able to execute any work.");
        } else {
            info!("Detected {} runtime(s):", detected.len());
            for rt in &detected {
                match &rt.version {
                    Some(ver) => info!("  ✓ {} — {} ({})", rt.name, rt.path, ver),
                    None => info!("  ✓ {} — {}", rt.name, rt.path),
                }
            }

            let runtime_csv = format_as_env_value(&detected);
            info!("Setting {}={}", env_var_name, runtime_csv);
            std::env::set_var(env_var_name, &runtime_csv);
            detected_runtimes = Some(detected);
        }
    }

    RuntimeBootstrapResult {
        runtimes_override,
        detected_runtimes,
    }
}

pub fn print_detect_only_report(
    env_var_name: &str,
    result: &RuntimeBootstrapResult,
    environment: &ChildProcessEnvironment,
) {
    if result.runtimes_override.is_some() {
        info!("--detect-only: re-running detection to show what is available on this system...");
        println!(
            "NOTE: {} is set — auto-detection was skipped during normal startup.",
            env_var_name
        );
        println!("      Showing what auto-detection would find on this system:");
        println!();

        let detected = detect_runtimes(environment);
        print_detection_report_for_env(env_var_name, &detected);
    } else if let Some(ref detected) = result.detected_runtimes {
        print_detection_report_for_env(env_var_name, detected);
    } else {
        let detected = detect_runtimes(environment);
        print_detection_report_for_env(env_var_name, &detected);
    }
}
