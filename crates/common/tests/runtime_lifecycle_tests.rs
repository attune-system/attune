#![cfg(unix)]

use attune_common::child_process_environment::ChildProcessEnvironment;
use attune_common::runtime_lifecycle::{RuntimeRecipe, RuntimeSource};
#[cfg(target_os = "linux")]
use attune_common::workload_sandbox::WorkloadSandbox;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn recipe(tool: &Path) -> RuntimeRecipe {
    serde_json::from_value(serde_json::json!({
        "tools":[{"name":"nova","binaries":[tool],"args":["--version"],
            "version_pattern":"Nova release=(?<version>[0-9.]+)","primary":true}],
        "extensions":["nova"],"workspace":true,
        "prepare":[{"command":["{tool_nova}","build","{source_file}","{output_dir}/program"],"timeout_seconds":2}],
        "launch":{"command":["{output_dir}/program"],"cwd":"{workspace_dir}"}
    })).unwrap()
}

fn environment(recipe: &RuntimeRecipe) -> ChildProcessEnvironment {
    ChildProcessEnvironment::from_parent(
        [
            ("PATH".into(), "/usr/bin:/bin".into()),
            (
                "ATTUNE_RUNTIME_RECIPE_DIGESTS".into(),
                recipe.digest().unwrap().into(),
            ),
            ("ATTUNE_API_TOKEN".into(), "dummy-secret".into()),
            ("ATTUNE__DATABASE__URL".into(), "dummy-database".into()),
        ],
        &[],
    )
    .unwrap()
}

fn fixture() -> (tempfile::TempDir, RuntimeRecipe) {
    let root = tempfile::tempdir().unwrap();
    let tool = root.path().join("nova");
    executable(&tool, "#!/bin/sh\n[ -z \"${ATTUNE_API_TOKEN+x}\" ] || exit 90\n[ -z \"${ATTUNE__DATABASE__URL+x}\" ] || exit 91\nif [ \"$1\" = --version ]; then printf 'Nova release=42.7\\n'; exit; fi\nif [ \"$1\" = build ]; then cp \"$2\" \"$3\"; chmod +x \"$3\"; exit; fi\nexit 92\n");
    let recipe = recipe(&tool);
    (root, recipe)
}

#[test]
fn recipe_versions_never_fall_back_to_another_owner_or_legacy_alias() {
    use attune_common::runtime_lifecycle::{available_versions, validate_version_mode};
    let (_, recipe) = fixture();
    let mut runtime: attune_common::models::Runtime = serde_json::from_value(serde_json::json!({
        "id":1,"ref":"a.python","pack":null,"pack_ref":"a","description":null,
        "name":"Python","aliases":["python"],"distributions":{},"installation":null,"installers":{},
        "execution_config":{"lifecycle":recipe},"auto_detected":false,"detection_config":{},
        "retired_at":null,"created":chrono::Utc::now(),"updated":chrono::Utc::now()
    }))
    .unwrap();
    let digest = recipe.digest().unwrap();
    let capabilities = serde_json::json!({
        "runtime_versions":{"python":["99.0"]},
        "detected_interpreters":[{"name":"python","version":"99.0"}],
        "runtime_toolchains":{
            "a.python":{"digest":digest,"versions":["3.11"],"names":["python"]},
            "b.python":{"digest":digest,"versions":["3.12"],"names":["python"]}
        }
    });
    assert_eq!(available_versions(Some(&capabilities), &runtime), ["3.11"]);
    runtime.r#ref = "b.python".into();
    assert_eq!(available_versions(Some(&capabilities), &runtime), ["3.12"]);
    runtime.r#ref = "absent.python".into();
    assert!(available_versions(Some(&capabilities), &runtime).is_empty());
    runtime.r#ref = "a.python".into();
    runtime.execution_config["lifecycle"]["env"] = serde_json::json!({"APP_REGION":"changed"});
    assert!(available_versions(Some(&capabilities), &runtime).is_empty());
    assert!(validate_version_mode(
        &runtime.execution_config,
        &serde_json::json!({"interpreter":{"binary":"python3"}})
    )
    .is_err());
    assert!(validate_version_mode(&serde_json::json!({}), &runtime.execution_config).is_err());
    assert!(validate_version_mode(&runtime.execution_config, &runtime.execution_config).is_ok());
}

#[tokio::test]
async fn inline_recipe_owns_an_empty_input_root_when_no_pack_exists() {
    let (root, recipe) = fixture();
    let target = recipe
        .prepare(
            &root.path().join("missing-ad-hoc-pack"),
            RuntimeSource::Inline("#!/bin/sh\nprintf inline-ok\n"),
            &root.path().join("generations"),
            &environment(&recipe),
        )
        .await
        .unwrap();
    assert!(target.generation.join("input").is_dir());
    assert!(!root.path().join("missing-ad-hoc-pack").exists());
    let output = tokio::process::Command::new(&target.command[0])
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"inline-ok");
    target.cleanup().await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires Linux Bubblewrap with unprivileged namespaces"]
async fn bubblewrap_recipe_enforces_build_and_invocation_paths() {
    let (root, mut recipe) = fixture();
    let alias_dir = root.path().join("aliases");
    std::fs::create_dir(&alias_dir).unwrap();
    let alias = alias_dir.join("nova");
    std::os::unix::fs::symlink(root.path().join("nova"), &alias).unwrap();
    recipe.tools[0].binaries = vec![alias.display().to_string()];
    recipe
        .env
        .insert("PYTHONPYCACHEPREFIX".into(), "{scratch_dir}/pycache".into());
    let environment = environment(&recipe).with_workload_sandbox(WorkloadSandbox::Bubblewrap);
    let pack = root.path().join("pack");
    std::fs::create_dir(&pack).unwrap();
    let manager_state = root.path().join("manager-state");
    std::fs::write(&manager_state, "owned fixture state").unwrap();
    let published_input = pack.join("immutable-input");
    std::fs::write(&published_input, "published fixture input").unwrap();
    let source = pack.join("action.nova");
    executable(
        &source,
        &format!(
            concat!(
                "#!/bin/sh\nset -e\n",
                "if cat '{}' 2>/dev/null; then exit 12; fi\n",
                "if (printf unexpected >> '{}') 2>/dev/null; then exit 15; fi\n",
                "if (printf unexpected > ./outside-scratch) 2>/dev/null; then exit 16; fi\n",
                "mkdir -p \"$PYTHONPYCACHEPREFIX\"\n",
                "printf writable > \"$PYTHONPYCACHEPREFIX/cache\"\n",
            ),
            manager_state.display(),
            published_input.display()
        ),
    );
    let tool = root.path().join("nova");
    let tool_script = std::fs::read_to_string(&tool).unwrap();
    executable(
        &tool,
        &tool_script.replacen(
            "#!/bin/sh\n",
            &format!(
                concat!(
                    "#!/bin/sh\n",
                    "if cat '{}' 2>/dev/null; then exit 13; fi\n",
                    "if [ \"$1\" = build ] && (printf unexpected >> '{}') 2>/dev/null; then exit 15; fi\n",
                ),
                manager_state.display(),
                published_input.display()
            ),
            1,
        ),
    );
    let generation_root = root.path().join("generations");
    let target = recipe
        .prepare(
            &pack,
            RuntimeSource::File(&source),
            &generation_root,
            &environment,
        )
        .await
        .unwrap();
    let cache = Path::new(&target.env["PYTHONPYCACHEPREFIX"]);
    assert!(!cache.starts_with(&target.generation));
    let mut command = std::process::Command::new(&target.command[0]);
    environment.apply(&mut command);
    command.envs(&target.env).current_dir(&target.working_dir);
    let command = target.sandbox_invocation(command, &environment).unwrap();
    let output = tokio::process::Command::from(command)
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(std::fs::read(cache.join("cache")).unwrap(), b"writable");
    assert_eq!(
        std::fs::read(&published_input).unwrap(),
        b"published fixture input"
    );
    target.cleanup().await.unwrap();
    assert_eq!(std::fs::read_dir(generation_root).unwrap().count(), 0);
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires Linux Bubblewrap with unprivileged namespaces and Python"]
async fn bubblewrap_python_recipe_keeps_import_bytecode_in_private_scratch() {
    let python = Path::new("/usr/bin/python3");
    if !python.is_file() {
        panic!("Python compatibility fixture requires /usr/bin/python3");
    }
    let recipe: RuntimeRecipe = serde_json::from_value(serde_json::json!({
        "tools": [{
            "name": "python",
            "binaries": [python],
            "args": ["--version"],
            "version_pattern": "Python (?<version>3\\.\\d+\\.\\d+)",
            "primary": true
        }],
        "launch": {"command": ["{tool_python}", "{source_file}"]},
        "env": {"PYTHONPYCACHEPREFIX": "{scratch_dir}/pycache"}
    }))
    .unwrap();
    let environment = environment(&recipe).with_workload_sandbox(WorkloadSandbox::Bubblewrap);
    let root = tempfile::tempdir().unwrap();
    let pack = root.path().join("pack");
    std::fs::create_dir(&pack).unwrap();
    let source = pack.join("action.py");
    std::fs::write(
        &source,
        "import fixture_dependency\nprint(fixture_dependency.value)\n",
    )
    .unwrap();
    std::fs::write(pack.join("fixture_dependency.py"), "value = 'imported'\n").unwrap();
    let target = recipe
        .prepare(
            &pack,
            RuntimeSource::File(&source),
            &root.path().join("generations"),
            &environment,
        )
        .await
        .unwrap();
    let mut command = std::process::Command::new(&target.command[0]);
    environment.apply(&mut command);
    command
        .args(&target.command[1..])
        .envs(&target.env)
        .current_dir(&target.working_dir);
    let command = target.sandbox_invocation(command, &environment).unwrap();
    let output = tokio::process::Command::from(command)
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"imported\n");
    assert!(!pack.join("__pycache__").exists());
    let cache = Path::new(&target.env["PYTHONPYCACHEPREFIX"]).join(pack.strip_prefix("/").unwrap());
    assert!(std::fs::read_dir(cache).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("fixture_dependency.")));
    target.cleanup().await.unwrap();
}

#[tokio::test]
async fn runtime_generation_teardown_requires_confirmed_process_group_exit() {
    use attune_common::runtime_lifecycle::stop_owned_process_group;
    assert!(stop_owned_process_group(None).await.is_err());
    let mut leader = tokio::process::Command::new("/bin/sleep")
        .arg("60")
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let group = leader.id().unwrap();
    let mut writer = tokio::process::Command::new("/bin/sleep")
        .arg("60")
        .process_group(group as i32)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    leader.kill().await.unwrap();
    leader.wait().await.unwrap();
    let (teardown, reaped) = tokio::join!(stop_owned_process_group(Some(group)), writer.wait());
    teardown.unwrap();
    assert!(!reaped.unwrap().success());
    assert_eq!(unsafe { libc::kill(-(group as i32), 0) }, -1);
}

#[tokio::test]
async fn cancelled_discovery_retains_probe_home_until_writer_teardown_is_confirmed() {
    let (root, recipe) = fixture();
    let ready = root.path().join("probe-ready");
    executable(
        &root.path().join("nova"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n%s\\n' \"$HOME\" \"$$\" > '{}'\nwhile :; do :; done\n",
            ready.display()
        ),
    );
    let env = environment(&recipe);
    let mut discovery = Box::pin(recipe.discover(&env));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            tokio::select! {
                result = &mut discovery => panic!("Probe exited before cancellation: {result:?}"),
                _ = tokio::time::sleep(std::time::Duration::from_millis(5)) => {
                    if std::fs::read_to_string(&ready).is_ok_and(|s| s.lines().count() == 2) {
                        break;
                    }
                }
            }
        }
    })
    .await
    .expect("Probe did not publish readiness");
    let state = std::fs::read_to_string(ready).unwrap();
    let mut lines = state.lines();
    let home = std::path::PathBuf::from(lines.next().unwrap());
    let pid: i32 = lines.next().unwrap().parse().unwrap();
    drop(discovery);
    assert!(
        home.is_dir(),
        "Cancellation must retain an uncertain writer's probe home"
    );
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if unsafe { libc::kill(pid, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("Cancelled probe was not killed and reaped");
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn unknown_toolchain_builds_a_native_target_without_mutating_release() {
    let (root, recipe) = fixture();
    let pack = root.path().join("pack");
    std::fs::create_dir(&pack).unwrap();
    let source = pack.join("main.nova");
    executable(
        &source,
        "#!/bin/sh\nread -r parameters\nprintf '%s\\n' \"$parameters\"\n",
    );
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o500)).unwrap();
    let env = environment(&recipe);
    let tools = recipe.discover(&env).await.unwrap();
    assert_eq!(tools["nova"].version, "42.7");
    let first = recipe
        .prepare(
            &pack,
            RuntimeSource::File(&source),
            &root.path().join("generations"),
            &env,
        )
        .await
        .unwrap();
    let second = recipe
        .prepare(
            &pack,
            RuntimeSource::File(&source),
            &root.path().join("generations"),
            &env,
        )
        .await
        .unwrap();
    assert_ne!(first.generation, second.generation);
    assert!(first.validate_selected_version("42.7").is_ok());
    assert!(first.validate_selected_version("42.8").is_err());
    assert_ne!(
        first
            .working_dir
            .join("main.nova")
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o200,
        0
    );
    assert_eq!(source.metadata().unwrap().permissions().mode() & 0o200, 0);
    assert!(Path::new(&first.command[0]).starts_with(&first.generation));
    assert!(!pack.join("output").exists());
    assert!(!first.env.keys().any(|key| key.starts_with("ATTUNE_")));
    assert_eq!(
        std::fs::read_to_string(&source).unwrap(),
        std::fs::read_to_string(first.working_dir.join("main.nova")).unwrap()
    );
    let generation = first.generation.clone();
    first.cleanup().await.unwrap();
    second.cleanup().await.unwrap();
    assert!(!generation.exists());
}

#[tokio::test]
async fn discovery_requires_exact_admission_before_running_any_command() {
    let (root, recipe) = fixture();
    let env = ChildProcessEnvironment::from_parent([], &[]).unwrap();
    assert!(recipe
        .discover(&env)
        .await
        .unwrap_err()
        .to_string()
        .contains("not admitted"));
    assert!(!root.path().join("generations").exists());
    let env = environment(&recipe);
    let mut edited = recipe.clone();
    edited.launch.command.push("different".into());
    assert!(edited
        .discover(&env)
        .await
        .unwrap_err()
        .to_string()
        .contains("not admitted"));
}

#[tokio::test]
async fn inline_source_is_private_and_uses_the_same_build_contract() {
    let (root, recipe) = fixture();
    let pack = root.path().join("pack");
    std::fs::create_dir(&pack).unwrap();
    let target = recipe
        .prepare(
            &pack,
            RuntimeSource::Inline("#!/bin/sh\nprintf 'inline'\n"),
            &root.path().join("generations"),
            &environment(&recipe),
        )
        .await
        .unwrap();
    assert!(std::fs::read_to_string(&target.command[0])
        .unwrap()
        .contains("inline"));
    assert_eq!(std::fs::read_dir(pack).unwrap().count(), 0);
    target.cleanup().await.unwrap();
}

#[tokio::test]
async fn source_and_workspace_symlink_escapes_are_rejected() {
    let (root, recipe) = fixture();
    let pack = root.path().join("pack");
    std::fs::create_dir(&pack).unwrap();
    let outside = root.path().join("outside");
    executable(&outside, "#!/bin/sh\nexit 0\n");
    let env = environment(&recipe);
    assert!(recipe
        .prepare(
            &pack,
            RuntimeSource::File(&outside),
            &root.path().join("generations"),
            &env
        )
        .await
        .is_err());
    std::os::unix::fs::symlink(&outside, pack.join("main.nova")).unwrap();
    assert!(recipe
        .prepare(
            &pack,
            RuntimeSource::File(&pack.join("main.nova")),
            &root.path().join("generations"),
            &env
        )
        .await
        .is_err());
}

#[tokio::test]
async fn build_failure_never_returns_a_target_or_discloses_command_output() {
    let (root, mut recipe) = fixture();
    let pack = root.path().join("pack");
    std::fs::create_dir(&pack).unwrap();
    executable(&pack.join("main.nova"), "#!/bin/sh\nexit 0\n");
    recipe.prepare[0].command = vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf dummy-registry-secret >&2; exit 17".into(),
    ];
    let error = recipe
        .prepare(
            &pack,
            RuntimeSource::File(&pack.join("main.nova")),
            &root.path().join("generations"),
            &environment(&recipe),
        )
        .await
        .err()
        .unwrap();
    assert!(!error.to_string().contains("dummy-registry-secret"));
    let generations = std::fs::read_dir(root.path().join("generations"))
        .unwrap()
        .count();
    assert_eq!(
        generations, 1,
        "failed generation is quarantined, never reused"
    );
}

#[test]
fn malformed_and_reserved_recipe_fields_are_rejected_without_secret_values() {
    let (_, mut recipe) = fixture();
    recipe
        .env
        .insert("attune_api_token".into(), "dummy-secret".into());
    let error = recipe.validate().unwrap_err().to_string();
    assert!(!error.contains("dummy-secret"));
    recipe.env.clear();
    recipe.prepare[0].timeout_seconds = 0;
    assert!(recipe.validate().is_err());
}

#[test]
fn bundled_toolchain_recipes_validate_and_hash_canonically() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for file in [
        "packs/core/runtimes/bun.yaml",
        "packs/core/runtimes/python_uv.yaml",
        "packs/core/runtimes/python_uv_project.yaml",
        "packs/core/runtimes/pypy.yaml",
        "packs/core/runtimes/deno.yaml",
        "docs/examples/runtime-gradle.yaml",
        "docs/examples/runtime-native-image.yaml",
    ] {
        let value: serde_json::Value =
            serde_yaml_ng::from_slice(&std::fs::read(base.join(file)).unwrap()).unwrap();
        let config = &value["execution_config"];
        attune_common::execution_env::validate_runtime_environment(config).unwrap();
        let recipe: RuntimeRecipe = serde_json::from_value(config["lifecycle"].clone()).unwrap();
        recipe.validate().unwrap();
        let roundtrip: RuntimeRecipe =
            serde_json::from_value(serde_json::to_value(&recipe).unwrap()).unwrap();
        assert_eq!(
            recipe.digest().unwrap(),
            roundtrip.digest().unwrap(),
            "{file}"
        );
    }
}

#[test]
fn service_smoke_operator_digests_match_the_complete_canonical_recipe() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/runtime-services");
    for name in ["fixture", "slow_fixture"] {
        let definition: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join(format!("{name}.json"))).unwrap())
                .unwrap();
        let value = &definition["execution_config"]["lifecycle"];
        let recipe: RuntimeRecipe = serde_json::from_value(value.clone()).unwrap();
        recipe.validate().unwrap();
        assert_eq!(*value, serde_json::to_value(&recipe).unwrap());
        assert_eq!(
            recipe.digest().unwrap(),
            std::fs::read_to_string(root.join(format!("{name}.sha256")))
                .unwrap()
                .trim()
        );
    }
}

#[test]
fn caller_environment_admission_is_bound_to_the_recipe_digest() {
    let (_, mut recipe) = fixture();
    let digest = recipe.digest().unwrap();
    recipe.permitted_caller_env = vec!["APP_REGION".into()];
    recipe.validate().unwrap();
    assert_ne!(digest, recipe.digest().unwrap());
    let config = serde_json::json!({"lifecycle":recipe});
    assert!(attune_common::execution_env::validate_caller_environment(
        &config,
        &serde_json::json!({"APP_REGION":"test"})
    )
    .is_ok());
    let mut broadened = config;
    broadened["permitted_caller_env"] = serde_json::json!(["APP_SECRET"]);
    assert!(attune_common::execution_env::validate_caller_environment(
        &broadened,
        &serde_json::json!({"APP_SECRET":"dummy"})
    )
    .is_err());
}

#[tokio::test]
async fn virtual_environment_launch_preserves_the_interpreter_symlink_path() {
    let root = tempfile::tempdir().unwrap();
    let pack = root.path().join("pack");
    std::fs::create_dir(&pack).unwrap();
    let source = pack.join("main.py");
    std::fs::write(&source, "import sys; print(sys.prefix)\n").unwrap();
    let recipe:RuntimeRecipe=serde_json::from_value(serde_json::json!({
        "tools":[{"name":"python","binaries":["/usr/bin/python3"],"args":["--version"],"version_pattern":"Python (?<version>[0-9.]+)","primary":true}],
        "prepare":[{"command":["{tool_python}","-m","venv","--without-pip","{env_dir}/venv"]}],
        "launch":{"command":["{env_dir}/venv/bin/python","{source_file}"]}
    })).unwrap();
    let env = environment(&recipe);
    let target = recipe
        .prepare(
            &pack,
            RuntimeSource::File(&source),
            &root.path().join("generations"),
            &env,
        )
        .await
        .unwrap();
    let mut command = tokio::process::Command::new(&target.command[0]);
    env.apply(command.as_std_mut());
    let output = command
        .args(&target.command[1..])
        .envs(&target.env)
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        target.generation.join("venv").display().to_string()
    );
    target.cleanup().await.unwrap();
}

#[tokio::test]
async fn setup_output_flood_is_bounded_and_never_disclosed() {
    let (root, mut recipe) = fixture();
    let pack = root.path().join("pack");
    std::fs::create_dir(&pack).unwrap();
    executable(&pack.join("main.nova"), "#!/bin/sh\nexit 0\n");
    recipe.prepare[0].command = vec![
        "/bin/sh".into(),
        "-c".into(),
        "while :; do printf 'dummy-secret-00000000000000000000000000000000'; done".into(),
    ];
    let error = recipe
        .prepare(
            &pack,
            RuntimeSource::File(&pack.join("main.nova")),
            &root.path().join("generations"),
            &environment(&recipe),
        )
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("output limit"));
    assert!(!error.to_string().contains("dummy-secret"));
}

#[tokio::test]
async fn candidate_runner_uses_shared_preparation_and_only_final_effect_credentials() {
    use attune_common::test_executor::{
        CandidateTestEnvironment, DiscoveryConfig, RunnerConfig, TestConfig, TestExecutor,
    };
    let (root, recipe) = fixture();
    let pack = root.path().join("pack");
    std::fs::create_dir(&pack).unwrap();
    executable(&pack.join("test.nova"),"#!/bin/sh\ntest \"$ATTUNE_CANDIDATE_EFFECTS_TOKEN\" = dummy-attempt-token || exit 9\nprintf 'passed\\n'\n");
    let generations = root.path().join("generations");
    let executor = TestExecutor::new(root.path().into())
        .with_runtime_lifecycle(environment(&recipe), generations.clone())
        .with_candidate_effects(CandidateTestEnvironment {
            url: "http://127.0.0.1:1/unused".into(),
            token: "dummy-attempt-token".into(),
        });
    let config = TestConfig {
        enabled: true,
        candidate_api_effects: true,
        discovery: DiscoveryConfig {
            method: "explicit".into(),
            path: None,
        },
        runners: std::collections::HashMap::from([(
            "generic".into(),
            RunnerConfig {
                lifecycle: Some(recipe),
                r#type: "runtime".into(),
                entry_point: "test.nova".into(),
                timeout: Some(5),
                result_format: Some("simple".into()),
            },
        )]),
        result_format: None,
        result_path: None,
        min_pass_rate: Some(1.0),
        on_failure: Some("block".into()),
    };
    let result = executor
        .execute_pack_tests_at(&pack, "example", "1.0.0", &config)
        .await;
    executor.process_owner().shutdown().await;
    let result = result.unwrap();
    assert_eq!(result.passed, 1);
    assert_eq!(result.failed, 0);
    assert_eq!(std::fs::read_dir(generations).unwrap().count(), 0);
}

#[tokio::test]
async fn candidate_runner_timeout_includes_preparation_and_quarantines_target() {
    use attune_common::test_executor::{DiscoveryConfig, RunnerConfig, TestConfig, TestExecutor};
    let (root, mut recipe) = fixture();
    let pack = root.path().join("pack");
    std::fs::create_dir(&pack).unwrap();
    executable(
        &pack.join("test.nova"),
        "#!/bin/sh\nprintf 'should-not-launch'\n",
    );
    recipe.prepare[0].command = vec!["/bin/sleep".into(), "30".into()];
    let generations = root.path().join("generations");
    let executor = TestExecutor::new(root.path().into())
        .with_runtime_lifecycle(environment(&recipe), generations.clone());
    let config = TestConfig {
        enabled: true,
        candidate_api_effects: false,
        discovery: DiscoveryConfig {
            method: "explicit".into(),
            path: None,
        },
        runners: std::collections::HashMap::from([(
            "fixture".into(),
            RunnerConfig {
                lifecycle: Some(recipe),
                r#type: "runtime".into(),
                entry_point: "test.nova".into(),
                timeout: Some(1),
                result_format: None,
            },
        )]),
        result_format: None,
        result_path: None,
        min_pass_rate: None,
        on_failure: Some("block".into()),
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        executor.execute_pack_tests_at(&pack, "example", "1.0.0", &config),
    )
    .await
    .expect("Preparation ignored runner timeout")
    .unwrap();
    executor.process_owner().shutdown().await;
    assert!(!config.accepts_result(&result));
    assert_eq!(std::fs::read_dir(generations).unwrap().count(), 1);
}
