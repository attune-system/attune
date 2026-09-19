use attune_cli::cli::{Cli, CliOutputFormat, Commands};
use attune_cli::commands::{pack::PackCommands, pack_release::ReleaseCommands};
use clap::Parser;
use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn offline(home: &Path) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("attune"));
    command
        .env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("ATTUNE_PROFILE", "does-not-exist")
        .env("ATTUNE_API_URL", "http://127.0.0.1:1");
    command
}

#[test]
fn release_cli_has_no_clap_conflicts_and_preserves_existing_output_flags() {
    for args in [
        vec![
            "attune",
            "pack",
            "release",
            "build",
            "source",
            "--output",
            "release.gz",
            "--format",
            "json",
        ],
        vec![
            "attune",
            "pack",
            "release",
            "verify",
            "release.gz",
            "--format",
            "json",
        ],
        vec![
            "attune",
            "pack",
            "release",
            "inspect",
            "release.gz",
            "--output",
            "json",
        ],
        vec!["attune", "pack", "list", "--output", "json"],
        vec!["attune", "--output", "yaml", "pack", "list"],
    ] {
        assert!(Cli::try_parse_from(args.clone()).is_ok(), "{args:?}");
    }
    assert!(Cli::try_parse_from(["attune", "pack", "release", "build", "source"]).is_err());
    assert!(Cli::try_parse_from([
        "attune", "--output", "json", "pack", "release", "build", "source"
    ])
    .is_err());
    assert!(Cli::try_parse_from(["attune", "pack", "list", "--output", "file.gz"]).is_err());
}

#[test]
fn build_verify_inspect_work_without_auth_or_config() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::create_dir_all(source.join("actions")).unwrap();
    fs::create_dir_all(source.join("dist")).unwrap();
    fs::write(source.join("pack.yaml"), "ref: offline\nversion: 1.0.0\n").unwrap();
    fs::write(source.join(".gitignore"), "dist/\n").unwrap();
    fs::write(
        source.join("actions/run.yaml"),
        "ref: offline.run\nrunner_type: native\nlaunch: {type: native, artifact: run}\n",
    )
    .unwrap();
    fs::write(source.join("dist/native"), b"never execute me").unwrap();
    let archive = dir.path().join("release.gz");
    let home = dir.path().join("unconfigured-home");
    let output = offline(&home)
        .args(["pack", "release", "build"])
        .arg(&source)
        .arg("--artifact")
        .arg(format!(
            "run[linux/amd64/static]={}/dist/native",
            source.display()
        ))
        .arg("--output")
        .arg(&archive)
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let built: Value = serde_json::from_slice(&output.stdout).unwrap();
    for verb in ["verify", "inspect"] {
        let output = offline(&home)
            .args(["pack", "release", verb])
            .arg(&archive)
            .args(["--format", "json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(built, result);
    }
    assert!(
        !home.exists(),
        "offline commands must not create config state"
    );
    let output = offline(&home)
        .args(["pack", "release", "build"])
        .arg(&source)
        .arg("--artifact")
        .arg(format!(
            "run[linux/amd64/static]={}/dist/native",
            source.display()
        ))
        .arg("--output")
        .arg(&archive)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "build must not overwrite an existing archive"
    );
    fs::write(&archive, b"invalid archive").unwrap();
    for verb in ["verify", "inspect"] {
        let output = offline(&home)
            .args(["pack", "release", verb])
            .arg(&archive)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn root_json_format_is_independent_of_the_archive_destination() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("pack.yaml"), "ref: flags\nversion: 1.0.0\n").unwrap();
    for (index, flags) in [vec!["--output", "json"], vec!["--json"], vec![]]
        .iter()
        .enumerate()
    {
        let destination = dir.path().join(format!("{index}.gz"));
        let mut command = offline(&dir.path().join("home"));
        command
            .args(flags)
            .args(["pack", "release", "build"])
            .arg(&source)
            .arg("--output")
            .arg(&destination);
        if flags.is_empty() {
            command.args(["--format", "json"]);
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            serde_json::from_slice::<Value>(&result.stdout).is_ok(),
            "not JSON: {}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert!(destination.is_file());
    }
}

#[test]
fn parsed_format_and_destination_values_are_distinct() {
    for (root_flags, local_flags, expected_json, expected_format) in [
        (
            vec!["--output", "json"],
            vec![],
            false,
            Some(CliOutputFormat::Json),
        ),
        (vec!["--json"], vec![], true, None),
        (
            vec![],
            vec!["--format", "json"],
            false,
            Some(CliOutputFormat::Json),
        ),
        (vec![], vec![], false, None),
    ] {
        let args = [
            vec!["attune"],
            root_flags,
            vec!["pack", "release", "build", "source", "--output", "json"],
            local_flags,
        ]
        .concat();
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.json, expected_json);
        assert!(cli.output == expected_format);
        let Commands::Pack {
            command:
                PackCommands::Release {
                    command:
                        ReleaseCommands::Build {
                            archive_output,
                            format,
                            ..
                        },
                },
        } = cli.command
        else {
            panic!("wrong command");
        };
        assert_eq!(archive_output, Path::new("json"));
        assert!(format == expected_format);
    }
    for args in [
        vec!["attune", "--output", "json", "pack", "list"],
        vec!["attune", "pack", "list", "--output", "json"],
        vec![
            "attune",
            "pack",
            "release",
            "inspect",
            "release.gz",
            "--output",
            "json",
        ],
    ] {
        assert!(Cli::try_parse_from(args).unwrap().output == Some(CliOutputFormat::Json));
    }
}
