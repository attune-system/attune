//! Dynamic Runtime Registration Module
//!
//! When the agent detects an interpreter on the local system (e.g., Ruby, Go, Perl)
//! that does not yet have a corresponding runtime entry in the database, this module
//! handles auto-registering it so that the normal runtime-loading pipeline in
//! `WorkerService::new()` picks it up.
//!
//! ## Registration Strategy
//!
//! For each detected runtime the agent found:
//!
//! 1. **Look up by name** in the database using alias-aware matching.
//! 2. **If found** → already registered (either from a pack YAML or a previous
//!    agent run). Nothing to do.
//! 3. **If not found** → search for a runtime *template* in loaded packs whose
//!    normalized name matches. Templates are pack-registered runtimes (e.g.,
//!    `core.ruby`) that provide the full `execution_config` needed to invoke
//!    the interpreter, manage environments, and install dependencies.
//! 4. **If a template is found** → clone it as an auto-detected runtime with
//!    `auto_detected = true` and populate `detection_config` with what the
//!    agent discovered (binary path, version, etc.).
//! 5. **If no template exists** → create a minimal runtime with just the
//!    detected interpreter binary path and file extension. This lets the agent
//!    execute simple scripts immediately, even without a full template.
//! 6. Mark all auto-registered runtimes with `auto_detected = true`.

use attune_common::error::Result;
use attune_common::models::runtime::Runtime;
use attune_common::repositories::runtime::{CreateRuntimeInput, RuntimeRepository};
use attune_common::repositories::{Create, FindByRef, List};

use serde_json::json;
use sqlx::PgPool;
use tracing::{debug, info, warn};

use crate::runtime_detect::DetectedRuntime;

/// Canonical file extensions for runtimes that the auto-detection module knows
/// about. Used when creating minimal runtime entries without a template.
fn default_file_extension(runtime_name: &str) -> Option<&'static str> {
    match runtime_name {
        "shell" => Some(".sh"),
        "python" => Some(".py"),
        "node" => Some(".js"),
        "ruby" => Some(".rb"),
        "go" => Some(".go"),
        "java" => Some(".java"),
        "perl" => Some(".pl"),
        "r" => Some(".R"),
        _ => None,
    }
}

/// Ensure that every detected runtime has a corresponding entry in the
/// `runtime` table. Runtimes that already exist (from pack loading or a
/// previous agent run) are left untouched. Missing runtimes are created
/// either from a matching pack template or as a minimal auto-detected entry.
///
/// This function should be called **before** `WorkerService::new()` so that
/// the normal runtime-loading pipeline finds all detected runtimes in the DB.
///
/// Returns the number of runtimes that were newly registered.
pub async fn auto_register_detected_runtimes(
    pool: &PgPool,
    detected: &[DetectedRuntime],
) -> Result<usize> {
    if detected.is_empty() {
        return Ok(0);
    }

    info!(
        "Checking {} detected runtime(s) for dynamic registration...",
        detected.len()
    );

    // Load all existing runtimes once to avoid repeated queries.
    let existing_runtimes = RuntimeRepository::list(pool).await.unwrap_or_default();

    let mut registered_count = 0;

    for detected_rt in detected {
        let canonical_name = detected_rt.name.to_ascii_lowercase();

        // Check if a runtime with a matching name already exists in the DB.
        // Primary: check if the detected name appears in any existing runtime's aliases.
        // Secondary: check if the ref ends with the canonical name (e.g., "core.ruby").
        let already_exists = existing_runtimes.iter().any(|r| {
            // Primary: check if the detected name is in this runtime's aliases
            r.aliases.iter().any(|a| a == &canonical_name)
            // Secondary: check if the ref ends with the canonical name (e.g., "core.ruby")
            || r.r#ref.ends_with(&format!(".{}", canonical_name))
        });

        if already_exists {
            debug!(
                "Runtime '{}' (canonical: '{}') already exists in database, skipping",
                detected_rt.name, canonical_name
            );
            continue;
        }

        // No existing runtime — try to find a template from loaded packs.
        // Templates are pack-registered runtimes whose normalized name matches
        // (e.g., `core.ruby` for detected runtime "ruby"). Since we already
        // checked `existing_runtimes` above and found nothing, we look for
        // runtimes by ref pattern. Common convention: `core.<name>`.
        let template_ref = format!("core.{}", canonical_name);
        let template = RuntimeRepository::find_by_ref(pool, &template_ref)
            .await
            .unwrap_or(None);

        let detection_config = build_detection_config(detected_rt);

        if let Some(tmpl) = template {
            // Clone the template as an auto-detected runtime.
            // The template already has the full execution_config, distributions, etc.
            // We just re-create it with auto_detected = true.
            info!(
                "Found template '{}' for detected runtime '{}', registering auto-detected clone",
                tmpl.r#ref, detected_rt.name
            );

            // Use a distinct ref so we don't collide with the template.
            let auto_ref = format!("auto.{}", canonical_name);

            // Check if the auto ref already exists (race condition / previous run)
            if RuntimeRepository::find_by_ref(pool, &auto_ref)
                .await
                .unwrap_or(None)
                .is_some()
            {
                debug!(
                    "Auto-detected runtime '{}' already registered from a previous run",
                    auto_ref
                );
                continue;
            }

            let input = CreateRuntimeInput {
                r#ref: auto_ref.clone(),
                pack: tmpl.pack,
                pack_ref: tmpl.pack_ref.clone(),
                description: Some(format!(
                    "Auto-detected {} runtime (from template {})",
                    detected_rt.name, tmpl.r#ref
                )),
                name: tmpl.name.clone(),
                aliases: tmpl.aliases.clone(),
                distributions: tmpl.distributions.clone(),
                installation: tmpl.installation.clone(),
                execution_config: build_execution_config_from_template(&tmpl, detected_rt),
                auto_detected: true,
                detection_config,
            };

            match RuntimeRepository::create(pool, input).await {
                Ok(rt) => {
                    info!(
                        "Auto-registered runtime '{}' (ID: {}) from template '{}'",
                        auto_ref, rt.id, tmpl.r#ref
                    );
                    registered_count += 1;
                }
                Err(e) => {
                    // Unique constraint violation is fine (concurrent agent start)
                    warn!("Failed to auto-register runtime '{}': {}", auto_ref, e);
                }
            }
        } else {
            // No template found — create a minimal runtime entry.
            info!(
                "No template found for detected runtime '{}', creating minimal entry",
                detected_rt.name
            );

            let auto_ref = format!("auto.{}", canonical_name);

            if RuntimeRepository::find_by_ref(pool, &auto_ref)
                .await
                .unwrap_or(None)
                .is_some()
            {
                debug!(
                    "Auto-detected runtime '{}' already registered from a previous run",
                    auto_ref
                );
                continue;
            }

            let execution_config = build_minimal_execution_config(detected_rt);

            let input = CreateRuntimeInput {
                r#ref: auto_ref.clone(),
                pack: None,
                pack_ref: None,
                description: Some(format!(
                    "Auto-detected {} runtime at {}",
                    detected_rt.name, detected_rt.path
                )),
                name: capitalize_runtime_name(&canonical_name),
                aliases: default_aliases(&canonical_name),
                distributions: build_minimal_distributions(detected_rt),
                installation: None,
                execution_config,
                auto_detected: true,
                detection_config,
            };

            match RuntimeRepository::create(pool, input).await {
                Ok(rt) => {
                    info!(
                        "Auto-registered minimal runtime '{}' (ID: {})",
                        auto_ref, rt.id
                    );
                    registered_count += 1;
                }
                Err(e) => {
                    warn!("Failed to auto-register runtime '{}': {}", auto_ref, e);
                }
            }
        }
    }

    if registered_count > 0 {
        info!(
            "Dynamic runtime registration complete: {} new runtime(s) registered",
            registered_count
        );
    } else {
        info!("Dynamic runtime registration complete: all detected runtimes already in database");
    }

    Ok(registered_count)
}

/// Build the `detection_config` JSONB value from a detected runtime.
/// This metadata records how the agent discovered this runtime, enabling
/// re-verification and diagnostics.
fn build_detection_config(detected: &DetectedRuntime) -> serde_json::Value {
    let mut config = json!({
        "detected_path": detected.path,
        "detected_name": detected.name,
    });

    if let Some(ref version) = detected.version {
        config["detected_version"] = json!(version);
    }

    config
}

/// Build an execution config based on a template runtime, but with the
/// detected interpreter path substituted in. This ensures the auto-detected
/// runtime uses the actual binary path found on the system.
fn build_execution_config_from_template(
    template: &Runtime,
    detected: &DetectedRuntime,
) -> serde_json::Value {
    let mut config = template.execution_config.clone();

    // If the template has an interpreter config, update the binary path
    // to the one we actually detected on this system.
    if let Some(interpreter) = config.get_mut("interpreter") {
        if let Some(obj) = interpreter.as_object_mut() {
            obj.insert("binary".to_string(), json!(detected.path));
        }
    }

    // If the template has an environment config with an interpreter_path
    // that uses a template variable, leave it as-is (it will be resolved
    // at execution time). But if it's a hardcoded absolute path, update it.
    if let Some(env) = config.get_mut("environment") {
        if let Some(obj) = env.as_object_mut() {
            if let Some(interp_path) = obj.get("interpreter_path") {
                if let Some(path_str) = interp_path.as_str() {
                    // Only leave template variables alone
                    if !path_str.contains('{') {
                        obj.insert("interpreter_path".to_string(), json!(detected.path));
                    }
                }
            }
        }
    }

    config
}

/// Build a minimal execution config for a runtime with no template.
/// This provides enough information for `ProcessRuntime` to invoke the
/// interpreter directly, without environment or dependency management.
fn build_minimal_execution_config(detected: &DetectedRuntime) -> serde_json::Value {
    let canonical = detected.name.to_ascii_lowercase();
    let file_ext = default_file_extension(&canonical);

    let mut config = json!({
        "interpreter": {
            "binary": detected.path,
            "args": [],
        }
    });

    if let Some(ext) = file_ext {
        config["interpreter"]["file_extension"] = json!(ext);
    }

    config
}

/// Build minimal distributions metadata for a runtime with no template.
/// Includes a basic verification command using the detected binary path.
fn build_minimal_distributions(detected: &DetectedRuntime) -> serde_json::Value {
    json!({
        "verification": {
            "commands": [
                {
                    "binary": &detected.path,
                    "args": ["--version"],
                    "exit_code": 0,
                    "priority": 1
                }
            ]
        }
    })
}

/// Default aliases for auto-detected runtimes that have no template.
/// These match what the core pack YAMLs declare but serve as fallback
/// when the template hasn't been loaded.
fn default_aliases(canonical_name: &str) -> Vec<String> {
    match canonical_name {
        "shell" => vec!["shell".into(), "bash".into(), "sh".into()],
        "python" => vec!["python".into(), "python3".into()],
        "node" => vec!["node".into(), "nodejs".into(), "node.js".into()],
        "ruby" => vec!["ruby".into(), "rb".into()],
        "go" => vec!["go".into(), "golang".into()],
        "java" => vec!["java".into(), "jdk".into(), "openjdk".into()],
        "perl" => vec!["perl".into(), "perl5".into()],
        "r" => vec!["r".into(), "rscript".into()],
        _ => vec![canonical_name.to_string()],
    }
}

/// Capitalize a runtime name for display (e.g., "ruby" → "Ruby", "r" → "R").
fn capitalize_runtime_name(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let upper: String = first.to_uppercase().collect();
            format!("{}{}", upper, chars.as_str())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_file_extension() {
        assert_eq!(default_file_extension("shell"), Some(".sh"));
        assert_eq!(default_file_extension("python"), Some(".py"));
        assert_eq!(default_file_extension("node"), Some(".js"));
        assert_eq!(default_file_extension("ruby"), Some(".rb"));
        assert_eq!(default_file_extension("go"), Some(".go"));
        assert_eq!(default_file_extension("java"), Some(".java"));
        assert_eq!(default_file_extension("perl"), Some(".pl"));
        assert_eq!(default_file_extension("r"), Some(".R"));
        assert_eq!(default_file_extension("unknown"), None);
    }

    #[test]
    fn test_capitalize_runtime_name() {
        assert_eq!(capitalize_runtime_name("ruby"), "Ruby");
        assert_eq!(capitalize_runtime_name("go"), "Go");
        assert_eq!(capitalize_runtime_name("r"), "R");
        assert_eq!(capitalize_runtime_name("perl"), "Perl");
        assert_eq!(capitalize_runtime_name("java"), "Java");
        assert_eq!(capitalize_runtime_name(""), "");
    }

    #[test]
    fn test_build_detection_config_with_version() {
        let detected = DetectedRuntime {
            name: "ruby".to_string(),
            path: "/usr/bin/ruby".to_string(),
            version: Some("3.3.0".to_string()),
        };

        let config = build_detection_config(&detected);
        assert_eq!(config["detected_path"], "/usr/bin/ruby");
        assert_eq!(config["detected_name"], "ruby");
        assert_eq!(config["detected_version"], "3.3.0");
    }

    #[test]
    fn test_build_detection_config_without_version() {
        let detected = DetectedRuntime {
            name: "perl".to_string(),
            path: "/usr/bin/perl".to_string(),
            version: None,
        };

        let config = build_detection_config(&detected);
        assert_eq!(config["detected_path"], "/usr/bin/perl");
        assert_eq!(config["detected_name"], "perl");
        assert!(config.get("detected_version").is_none());
    }

    #[test]
    fn test_build_minimal_execution_config() {
        let detected = DetectedRuntime {
            name: "ruby".to_string(),
            path: "/usr/bin/ruby".to_string(),
            version: Some("3.3.0".to_string()),
        };

        let config = build_minimal_execution_config(&detected);
        assert_eq!(config["interpreter"]["binary"], "/usr/bin/ruby");
        assert_eq!(config["interpreter"]["file_extension"], ".rb");
        assert_eq!(config["interpreter"]["args"], json!([]));
    }

    #[test]
    fn test_build_minimal_execution_config_unknown_runtime() {
        let detected = DetectedRuntime {
            name: "custom".to_string(),
            path: "/opt/custom/bin/custom".to_string(),
            version: None,
        };

        let config = build_minimal_execution_config(&detected);
        assert_eq!(config["interpreter"]["binary"], "/opt/custom/bin/custom");
        // Unknown runtime has no file extension
        assert!(config["interpreter"].get("file_extension").is_none());
    }

    #[test]
    fn test_build_minimal_distributions() {
        let detected = DetectedRuntime {
            name: "ruby".to_string(),
            path: "/usr/bin/ruby".to_string(),
            version: Some("3.3.0".to_string()),
        };

        let distros = build_minimal_distributions(&detected);
        let commands = distros["verification"]["commands"].as_array().unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0]["binary"], "/usr/bin/ruby");
    }

    #[test]
    fn test_build_execution_config_from_template_updates_binary() {
        let template = Runtime {
            id: 1,
            r#ref: "core.ruby".to_string(),
            pack: Some(1),
            pack_ref: Some("core".to_string()),
            description: Some("Ruby Runtime".to_string()),
            name: "Ruby".to_string(),
            aliases: vec!["ruby".to_string(), "rb".to_string()],
            distributions: json!({}),
            installation: None,
            installers: json!({}),
            execution_config: json!({
                "interpreter": {
                    "binary": "ruby",
                    "args": [],
                    "file_extension": ".rb"
                },
                "env_vars": {
                    "GEM_HOME": "{env_dir}/gems"
                }
            }),
            auto_detected: false,
            detection_config: json!({}),
            retired_at: None,
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
        };

        let detected = DetectedRuntime {
            name: "ruby".to_string(),
            path: "/usr/local/bin/ruby3.3".to_string(),
            version: Some("3.3.0".to_string()),
        };

        let config = build_execution_config_from_template(&template, &detected);

        // Binary should be updated to the detected path
        assert_eq!(config["interpreter"]["binary"], "/usr/local/bin/ruby3.3");
        // Other fields should be preserved from the template
        assert_eq!(config["interpreter"]["file_extension"], ".rb");
        assert_eq!(config["env_vars"]["GEM_HOME"], "{env_dir}/gems");
    }

    #[test]
    fn test_build_execution_config_from_template_preserves_template_vars() {
        let template = Runtime {
            id: 1,
            r#ref: "core.python".to_string(),
            pack: Some(1),
            pack_ref: Some("core".to_string()),
            description: None,
            name: "Python".to_string(),
            aliases: vec!["python".to_string(), "python3".to_string()],
            distributions: json!({}),
            installation: None,
            installers: json!({}),
            execution_config: json!({
                "interpreter": {
                    "binary": "python3",
                    "file_extension": ".py"
                },
                "environment": {
                    "interpreter_path": "{env_dir}/bin/python3",
                    "create_command": ["python3", "-m", "venv", "{env_dir}"]
                }
            }),
            auto_detected: false,
            detection_config: json!({}),
            retired_at: None,
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
        };

        let detected = DetectedRuntime {
            name: "python".to_string(),
            path: "/usr/bin/python3.12".to_string(),
            version: Some("3.12.1".to_string()),
        };

        let config = build_execution_config_from_template(&template, &detected);

        // Binary should be updated
        assert_eq!(config["interpreter"]["binary"], "/usr/bin/python3.12");
        // Template-variable interpreter_path should be preserved (contains '{')
        assert_eq!(
            config["environment"]["interpreter_path"],
            "{env_dir}/bin/python3"
        );
    }
}
