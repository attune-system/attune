//! Pure parser regressions. No profiles, network, database, or environment mutation.
use attune_cli::{
    cli::{Cli, Commands},
    commands::{
        component_config::ConfigurationCommands, pack::PackCommands, sensor::SensorCommands,
        trigger::TriggerCommands, trigger_rollout::RolloutCommands,
    },
};
use clap::Parser;

#[test]
fn pack_and_sensor_resets_are_explicit_commands_without_json() {
    let pack = Cli::try_parse_from([
        "attune", "pack", "config", "reset", "example", "token", "--yes",
    ])
    .unwrap();
    assert!(matches!(pack.command, Commands::Pack {
        command: PackCommands::Config { command: ConfigurationCommands::Reset { fields, yes:true, .. } }
    } if fields == ["token"]));
    let sensor = Cli::try_parse_from([
        "attune",
        "sensor",
        "config",
        "reset",
        "example.reader",
        "interval",
        "--dry-run",
    ])
    .unwrap();
    assert!(matches!(
        sensor.command,
        Commands::Sensor {
            command: SensorCommands::Config {
                command: ConfigurationCommands::Reset { dry_run: true, .. }
            }
        }
    ));
    assert!(
        Cli::try_parse_from(["attune", "pack", "config", "reset", "example", "--yes"]).is_err()
    );
}

#[test]
fn json_output_flag_does_not_conflict_with_json_override_values() {
    let parsed = Cli::try_parse_from([
        "attune",
        "pack",
        "config",
        "set",
        "example",
        "--set-json",
        "count=2",
        "--json",
    ])
    .unwrap();
    assert!(parsed.json);
    assert!(matches!(
        parsed.command,
        Commands::Pack {
            command: PackCommands::Config {
                command: ConfigurationCommands::Set { .. }
            }
        }
    ));
}

#[test]
fn rollout_confirmation_does_not_set_incompatible_acknowledgement() {
    let parsed = Cli::try_parse_from([
        "attune",
        "trigger",
        "rollout",
        "apply",
        "example.tick",
        "--clear-out-schema",
        "--yes",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Commands::Trigger {
            command: TriggerCommands::Rollout {
                command: RolloutCommands::Apply {
                    yes: true,
                    acknowledge_incompatible_rollout: false,
                    ..
                }
            }
        }
    ));
    let parsed = Cli::try_parse_from([
        "attune",
        "trigger",
        "rollout",
        "apply",
        "example.tick",
        "--clear-out-schema",
        "--yes",
        "--acknowledge-incompatible-rollout",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Commands::Trigger {
            command: TriggerCommands::Rollout {
                command: RolloutCommands::Apply {
                    acknowledge_incompatible_rollout: true,
                    ..
                }
            }
        }
    ));
    assert!(Cli::try_parse_from([
        "attune",
        "trigger",
        "rollout",
        "apply",
        "example.tick",
        "--clear-out-schema",
        "--force"
    ])
    .is_err());
}

#[test]
fn child_help_exposes_noninteractive_examples() {
    for arguments in [
        vec!["attune", "pack", "config", "reset", "--help"],
        vec!["attune", "sensor", "config", "set", "--help"],
        vec!["attune", "trigger", "rollout", "preflight", "--help"],
        vec!["attune", "trigger", "rollout", "apply", "--help"],
    ] {
        let help = Cli::try_parse_from(arguments).err().unwrap().to_string();
        assert!(help.contains("Examples:"));
    }
}
