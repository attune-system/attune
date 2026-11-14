//! The normal test harness never applies protection; only clean owned subprocesses do.
#[cfg(target_os = "linux")]
mod linux {
    use std::io::{Read, Write};
    use std::process::{Child, Command, Stdio};

    use attune_common::child_process_environment::ChildProcessEnvironment;
    use rustix::process::{dumpable_behavior, getuid, DumpableBehavior};
    use serde_json::{json, Value};

    const DUMMY: &str = "ATTUNE_PARENT_DUMMY=owned-not-a-real-secret";
    const INPUT: &str = r#"{"message":"ordinary stdin JSON"}"#;

    // Also reap children on assertion/IO failure; no background reader survives
    // the owned process. All output is small and bounded by this helper.
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            self.0.wait().expect("reap owned helper");
        }
    }

    fn invoke(command: &mut Command, input: &str) -> String {
        let mut child = OwnedChild(
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("spawn owned helper"),
        );
        child
            .0
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let mut output = String::new();
        child
            .0
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert!(child.0.wait().unwrap().success(), "owned helper failed");
        output
    }

    fn reader(parent: &str, uid: &str) {
        assert_eq!(getuid().as_raw().to_string(), uid, "must be same UID");
        assert!(std::env::var_os("ATTUNE_PARENT_DUMMY").is_none());
        assert!(std::env::var_os("ATTUNE_API_TOKEN").is_none());
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        let _: Value = serde_json::from_str(&input).unwrap();
        let environ = std::fs::read(format!("/proc/{parent}/environ"));
        if let Ok(bytes) = &environ {
            assert!(
                bytes
                    .split(|byte| *byte == 0)
                    .any(|entry| entry == DUMMY.as_bytes()),
                "readable owned initial environment must contain the dummy"
            );
        }
        let has_dummy = environ.as_ref().is_ok_and(|bytes| {
            bytes
                .split(|byte| *byte == 0)
                .any(|entry| entry == DUMMY.as_bytes())
        });
        let results: Vec<Value> = ["environ", "maps", "mem"]
            .into_iter()
            .map(
                |file| match std::fs::File::open(format!("/proc/{parent}/{file}")) {
                    Ok(_) => json!({"file": file, "readable": true}),
                    Err(error) => {
                        json!({"file": file, "readable": false, "errno": error.raw_os_error()})
                    }
                },
            )
            .collect();
        println!(
            "PROBE:{}",
            json!({"input": input, "has_dummy": has_dummy, "probes": results})
        );
    }

    fn parent(protected: bool) {
        assert_eq!(
            std::env::var("ATTUNE_PARENT_DUMMY").unwrap(),
            "owned-not-a-real-secret"
        );
        if protected {
            attune_common::parent_process_protection::protect_service_parent().unwrap();
            assert_eq!(dumpable_behavior().unwrap(), DumpableBehavior::NotDumpable);
        }
        let mut command = Command::new(std::env::current_exe().unwrap());
        ChildProcessEnvironment::capture(&[])
            .unwrap()
            .apply(&mut command);
        command
            .args(["--exact", "linux::subprocess_helper", "--nocapture"])
            .env("ATTUNE_PARENT_PROTECTION_TEST_MODE", "reader")
            .env(
                "ATTUNE_PARENT_PROTECTION_TEST_PID",
                std::process::id().to_string(),
            )
            .env(
                "ATTUNE_PARENT_PROTECTION_TEST_UID",
                getuid().as_raw().to_string(),
            );
        let output = invoke(&mut command, INPUT);
        let probe = output
            .lines()
            .find_map(|line| line.split_once("PROBE:").map(|(_, value)| value))
            .expect("reader report");
        let result: Value = serde_json::from_str(probe).unwrap();
        assert_eq!(result["input"], INPUT);
        if protected {
            for probe in result["probes"].as_array().unwrap() {
                assert_eq!(probe["readable"], false, "protected parent: {probe}");
                let errno = probe["errno"].as_i64().unwrap();
                assert!(errno == 13 || errno == 1, "expected access denial: {probe}");
            }
            assert!(!result["has_dummy"].as_bool().unwrap());
            assert_eq!(dumpable_behavior().unwrap(), DumpableBehavior::NotDumpable);
        } else if !result["has_dummy"].as_bool().unwrap() {
            eprintln!("Baseline precondition: host already restricts dummy parent environ; no exploit demonstrated");
        } else {
            eprintln!("Baseline: same-UID child read owned dummy parent initial environment");
        }
        println!(
            "protected={protected}, observed_dumpability={:?}, reader={result}",
            dumpable_behavior().unwrap()
        );
    }

    #[test]
    fn subprocess_helper() {
        match std::env::var("ATTUNE_PARENT_PROTECTION_TEST_MODE").as_deref() {
            Ok("reader") => reader(
                &std::env::var("ATTUNE_PARENT_PROTECTION_TEST_PID").unwrap(),
                &std::env::var("ATTUNE_PARENT_PROTECTION_TEST_UID").unwrap(),
            ),
            Ok("baseline") => parent(false),
            Ok("protected") => parent(true),
            _ => {} // Normal test discovery/execution does not change process state.
        }
    }

    #[test]
    fn owned_parent_procfs_and_stdin() {
        let before = dumpable_behavior().unwrap();
        for mode in ["baseline", "protected"] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .env_clear()
                .env("ATTUNE_PARENT_DUMMY", "owned-not-a-real-secret")
                .env("ATTUNE_PARENT_PROTECTION_TEST_MODE", mode)
                .args(["--exact", "linux::subprocess_helper", "--nocapture"]);
            print!("{}", invoke(&mut command, ""));
        }
        assert_eq!(dumpable_behavior().unwrap(), before, "driver state changed");
    }
}
