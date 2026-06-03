use clap::{Parser, Subcommand};
use std::process;

use attune_cli::{commands, config, output};
use commands::{
    action::{handle_action_command, ActionCommands},
    artifact::ArtifactCommands,
    audit::AuditCommands,
    auth::AuthCommands,
    config::ConfigCommands,
    execution::ExecutionCommands,
    key::KeyCommands,
    pack::PackCommands,
    queue::{handle_queue_command, QueueCommands},
    rule::RuleCommands,
    sensor::SensorCommands,
    trigger::TriggerCommands,
    workflow::WorkflowCommands,
};

#[derive(Parser)]
#[command(name = "attune")]
#[command(author, version, about = "Attune CLI - Event-driven automation platform", long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    /// Profile to use (overrides config)
    #[arg(short = 'p', long, env = "ATTUNE_PROFILE", global = true)]
    profile: Option<String>,

    /// API endpoint URL (overrides config)
    #[arg(long, env = "ATTUNE_API_URL", global = true)]
    api_url: Option<String>,

    /// Output format
    #[arg(long, value_enum, global = true, conflicts_with_all = ["json", "yaml"])]
    output: Option<output::OutputFormat>,

    /// Output as JSON (shorthand for --output json)
    #[arg(short = 'j', long, global = true, conflicts_with_all = ["output", "yaml"])]
    json: bool,

    /// Output as YAML (shorthand for --output yaml)
    #[arg(short = 'y', long, global = true, conflicts_with_all = ["output", "json"])]
    yaml: bool,

    /// Verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Authentication commands
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },
    /// Pack management
    Pack {
        #[command(subcommand)]
        command: PackCommands,
    },
    /// Action management and execution
    Action {
        #[command(subcommand)]
        command: ActionCommands,
    },
    /// Rule management
    Rule {
        #[command(subcommand)]
        command: RuleCommands,
    },
    /// Work queue management
    Queue {
        #[command(subcommand)]
        command: QueueCommands,
    },
    /// Key/secret management
    Key {
        #[command(subcommand)]
        command: KeyCommands,
    },
    /// Execution monitoring
    Execution {
        #[command(subcommand)]
        command: ExecutionCommands,
    },
    /// Workflow management
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommands,
    },
    /// Trigger management
    Trigger {
        #[command(subcommand)]
        command: TriggerCommands,
    },
    /// Sensor management
    Sensor {
        #[command(subcommand)]
        command: SensorCommands,
    },
    /// Artifact management (list, upload, download, delete)
    Artifact {
        #[command(subcommand)]
        command: ArtifactCommands,
    },
    /// Audit log queries (list, show, chain)
    Audit {
        #[command(subcommand)]
        command: AuditCommands,
    },
    /// Configuration management
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Run an action (shortcut for 'action execute')
    Run {
        /// Action reference (pack.action)
        action_ref: String,

        /// Action parameters in key=value format
        #[arg(long)]
        param: Vec<String>,

        /// Parameters as JSON string
        #[arg(long, conflicts_with = "param")]
        params_json: Option<String>,

        /// Worker label selector as JSON (e.g. '{"pool":"gpu"}')
        #[arg(long)]
        worker_selector: Option<String>,

        /// Worker tolerations as JSON array
        #[arg(long)]
        worker_tolerations: Option<String>,

        /// Worker affinity as JSON object
        #[arg(long)]
        worker_affinity: Option<String>,

        /// Watch execution until it completes
        #[arg(short, long)]
        watch: bool,

        /// Timeout in seconds when watching (default: 300)
        #[arg(long, default_value = "300", requires = "watch")]
        timeout: u64,

        /// Notifier WebSocket base URL (e.g. ws://localhost:8081).
        /// Derived from --api-url automatically when not set.
        #[arg(long, requires = "watch")]
        notifier_url: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    // Install HMAC-only JWT crypto provider (must be before any token operations)
    attune_common::auth::install_crypto_provider();

    let cli = Cli::parse();

    // Initialize logging
    if cli.verbose {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    }

    // Determine output format: explicit CLI flags > config file > default (table)
    let cli_override = if cli.json {
        Some(output::OutputFormat::Json)
    } else if cli.yaml {
        Some(output::OutputFormat::Yaml)
    } else {
        cli.output
    };
    let config_for_format =
        config::CliConfig::load_with_profile(cli.profile.as_deref()).unwrap_or_default();
    let output_format = config_for_format.effective_format(cli_override);

    let result = match cli.command {
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
            handle_queue_command(&cli.profile, command, &cli.api_url, output_format).await
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
            watch,
            timeout,
            notifier_url,
        } => {
            // Delegate to action execute command
            handle_action_command(
                &cli.profile,
                ActionCommands::Execute {
                    action_ref,
                    param,
                    params_json,
                    worker_selector,
                    worker_tolerations,
                    worker_affinity,
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
