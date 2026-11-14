use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run(binary: &str, input: &[u8]) -> Output {
    let mut child = Command::new(binary)
        .env_clear()
        .env("PATH", "/no-interpreters-or-tools")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn echo_preserves_every_message_byte_without_tools_or_credentials() {
    let message = "-n \\ quotes '\" 雪\nmessage=changed\nexit_code=9\n\0\n";
    for message in [message, "-n", "no trailing newline", ""] {
        let result = run(
            env!("CARGO_BIN_EXE_attune-core-echo"),
            &serde_json::to_vec(&serde_json::json!({"message":message})).unwrap(),
        );
        assert!(result.status.success());
        assert_eq!(result.stdout, message.as_bytes());
        assert!(result.stderr.is_empty());
    }
}

#[test]
fn text_cannot_replace_exit_code_or_sleep_duration() {
    let message = "雪\nexit_code=9\nseconds=3600\n";
    for (binary, input) in [
        (
            env!("CARGO_BIN_EXE_attune-core-noop"),
            serde_json::json!({"message":message,"exit_code":0}),
        ),
        (
            env!("CARGO_BIN_EXE_attune-core-sleep"),
            serde_json::json!({"message":message,"seconds":0}),
        ),
    ] {
        let result = run(binary, &serde_json::to_vec(&input).unwrap());
        assert!(result.status.success());
        assert!(String::from_utf8(result.stdout).unwrap().contains(message));
    }
    assert_eq!(
        run(
            env!("CARGO_BIN_EXE_attune-core-noop"),
            br#"{"exit_code":7}"#
        )
        .status
        .code(),
        Some(7)
    );
}

#[test]
fn invalid_values_and_old_physical_lines_fail_closed() {
    for binary in [
        env!("CARGO_BIN_EXE_attune-core-echo"),
        env!("CARGO_BIN_EXE_attune-core-sleep"),
        env!("CARGO_BIN_EXE_attune-core-noop"),
    ] {
        for input in [
            "message=a\nexit_code=0\n",
            "[]",
            "null",
            "{\"message\":2}",
            "{}{}",
        ] {
            let result = run(binary, input.as_bytes());
            assert!(!result.status.success(), "{binary}: {input}");
            assert!(result.stdout.is_empty());
        }
    }
    for (binary, key, invalid) in [
        (
            env!("CARGO_BIN_EXE_attune-core-sleep"),
            "seconds",
            vec![
                serde_json::json!(-1),
                serde_json::json!(3601),
                serde_json::json!(true),
                serde_json::json!("0"),
                serde_json::json!(0.5),
            ],
        ),
        (
            env!("CARGO_BIN_EXE_attune-core-noop"),
            "exit_code",
            vec![
                serde_json::json!(-1),
                serde_json::json!(256),
                serde_json::json!(true),
                serde_json::json!("0"),
            ],
        ),
    ] {
        for value in invalid {
            let input = serde_json::json!({key:value});
            assert!(!run(binary, &serde_json::to_vec(&input).unwrap())
                .status
                .success());
        }
    }
}
