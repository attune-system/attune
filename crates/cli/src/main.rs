use clap::Parser;
use std::process;

use attune_cli::cli::{Cli, CliOutputFormat, Commands, CompletionCommand, CompletionShell};
use attune_cli::commands::cache::{handle_cache_command, CacheOutput};
use attune_cli::{commands, config, output};

#[tokio::main]
async fn main() {
    // Install HMAC-only JWT crypto provider (must be before any token operations)
    attune_common::auth::install_crypto_provider();

    let cli = Cli::parse();

    // Completion is deliberately read-only. In particular, avoid the normal
    // configuration/output initialization because it creates a default config.
    match &cli.command {
        Commands::Completion { command } => {
            handle_completion(*command).unwrap_or_else(|error| {
                eprintln!("Error: {error}");
                process::exit(1);
            });
            return;
        }
        Commands::Complete { cursor, words } => {
            attune_cli::completion::print_candidates(
                words,
                cursor.unwrap_or(words.len().saturating_sub(1)),
            )
            .await;
            return;
        }
        _ => {}
    }

    // Initialize logging
    if cli.verbose {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    }

    // Determine output format: explicit CLI flags > config file > default (table)
    let cli_override = if cli.json {
        Some(CliOutputFormat::Json)
    } else if cli.yaml {
        Some(CliOutputFormat::Yaml)
    } else {
        cli.output
    };
    let cache_output = match cli_override {
        Some(CliOutputFormat::Ndjson) => CacheOutput::Ndjson,
        _ => CacheOutput::Standard,
    };
    let cli_override = cli_override
        .filter(|format| *format != CliOutputFormat::Ndjson)
        .map(output::OutputFormat::from);
    let config_for_format =
        config::CliConfig::load_with_profile(cli.profile.as_deref()).unwrap_or_default();
    let output_format = config_for_format.effective_format(cli_override);

    let result = match cli.command {
        Commands::Completion { command } => handle_completion(command),
        Commands::Complete { cursor, words } => {
            attune_cli::completion::print_candidates(
                &words,
                cursor.unwrap_or(words.len().saturating_sub(1)),
            )
            .await;
            Ok(())
        }
        Commands::Cache { command } => {
            handle_cache_command(
                &cli.profile,
                command,
                &cli.api_url,
                output_format,
                cache_output,
            )
            .await
        }
        _ if matches!(cache_output, CacheOutput::Ndjson) => Err(anyhow::anyhow!(
            "--output ndjson is only supported by 'attune cache entry scan --all'"
        )),
        Commands::Auth { command } => {
            commands::auth::handle_auth_command(&cli.profile, command, &cli.api_url, output_format)
                .await
        }
        Commands::Pack { command } => {
            commands::pack::handle_pack_command(&cli.profile, command, &cli.api_url, output_format)
                .await
        }
        Commands::Action { command } => {
            commands::action::handle_action_command(
                &cli.profile,
                command,
                &cli.api_url,
                output_format,
            )
            .await
        }
        Commands::Rule { command } => {
            commands::rule::handle_rule_command(&cli.profile, command, &cli.api_url, output_format)
                .await
        }
        Commands::Queue { command } => {
            commands::queue::handle_queue_command(
                &cli.profile,
                command,
                &cli.api_url,
                output_format,
            )
            .await
        }
        Commands::Policy { command } => {
            commands::policy::handle_policy_command(
                &cli.profile,
                command,
                &cli.api_url,
                output_format,
            )
            .await
        }
        Commands::Key { command } => {
            commands::key::handle_key_command(&cli.profile, command, &cli.api_url, output_format)
                .await
        }
        Commands::Execution { command } => {
            commands::execution::handle_execution_command(
                &cli.profile,
                command,
                &cli.api_url,
                output_format,
            )
            .await
        }
        Commands::Workflow { command } => {
            commands::workflow::handle_workflow_command(
                &cli.profile,
                command,
                &cli.api_url,
                output_format,
            )
            .await
        }
        Commands::Trigger { command } => {
            commands::trigger::handle_trigger_command(
                &cli.profile,
                command,
                &cli.api_url,
                output_format,
            )
            .await
        }
        Commands::Sensor { command } => {
            commands::sensor::handle_sensor_command(
                &cli.profile,
                command,
                &cli.api_url,
                output_format,
            )
            .await
        }
        Commands::Artifact { command } => {
            commands::artifact::handle_artifact_command(
                &cli.profile,
                command,
                &cli.api_url,
                output_format,
            )
            .await
        }
        Commands::Audit { command } => {
            commands::audit::handle_audit_command(
                &cli.profile,
                command,
                &cli.api_url,
                output_format,
            )
            .await
        }
        Commands::Config { command } => {
            commands::config::handle_config_command(&cli.profile, command, output_format).await
        }
        Commands::Run {
            action_ref,
            param,
            params_json,
            worker_selector,
            worker_tolerations,
            worker_affinity,
            execution_timeout,
            watch,
            timeout,
            notifier_url,
        } => {
            // Delegate to action execute command
            commands::action::handle_action_command(
                &cli.profile,
                commands::action::ActionCommands::Execute {
                    action_ref,
                    param,
                    params_json,
                    worker_selector,
                    worker_tolerations,
                    worker_affinity,
                    execution_timeout,
                    watch,
                    timeout,
                    notifier_url,
                },
                &cli.api_url,
                output_format,
            )
            .await
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

fn handle_completion(command: CompletionCommand) -> anyhow::Result<()> {
    match command {
        CompletionCommand::Bash => print_completion(CompletionShell::Bash),
        CompletionCommand::Fish => print_completion(CompletionShell::Fish),
        CompletionCommand::Zsh => print_completion(CompletionShell::Zsh),
        CompletionCommand::PowerShell => {
            print!("{}", attune_cli::completion::powershell_completion_script())
        }
        CompletionCommand::Install { shell } => {
            let path = attune_cli::completion::install(shell)?;
            println!(
                "Installed {} completion to {}",
                shell_name(shell),
                path.display()
            );
            if matches!(shell, CompletionShell::Zsh) {
                println!("Add this to ~/.zshrc, then restart Zsh:");
                println!("fpath=(~/.zsh/completions $fpath)");
                println!("autoload -Uz compinit && compinit");
            }
        }
    }
    Ok(())
}

fn shell_name(shell: CompletionShell) -> &'static str {
    match shell {
        CompletionShell::Bash => "Bash",
        CompletionShell::Fish => "Fish",
        CompletionShell::Zsh => "Zsh",
    }
}

fn print_completion(shell: CompletionShell) {
    match shell {
        CompletionShell::Bash => {
            print!("{}", attune_cli::completion::bash_completion_script())
        }
        CompletionShell::Fish => {
            print!("{}", attune_cli::completion::fish_completion_script())
        }
        CompletionShell::Zsh => {
            print!("{}", attune_cli::completion::zsh_completion_script())
        }
    }
}
