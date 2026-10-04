use crate::manual_execution::ManualExecutionOptions;
use clap::{Parser, Subcommand, ValueEnum};

use crate::commands::{
    action::ActionCommands, artifact::ArtifactCommands, audit::AuditCommands, auth::AuthCommands,
    cache::CacheCommands, config::ConfigCommands, execution::ExecutionCommands,
    identity::IdentityCommands, inquiry::InquiryCommands, key::KeyCommands, pack::PackCommands,
    permission::PermissionCommands, policy::PolicyCommands, queue::QueueCommands,
    rule::RuleCommands, sensor::SensorCommands, trigger::TriggerCommands,
    workflow::WorkflowCommands,
};

#[derive(Parser)]
#[command(name = "attune")]
#[command(author, version, about = "Attune CLI - Event-driven automation platform", long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    /// Profile to use (overrides config)
    #[arg(short = 'p', long, env = "ATTUNE_PROFILE", global = true)]
    pub profile: Option<String>,

    /// API endpoint URL (overrides config)
    #[arg(long, env = "ATTUNE_API_URL", global = true)]
    pub api_url: Option<String>,

    /// Output format
    #[arg(long, value_enum, global = true, conflicts_with_all = ["json", "yaml"])]
    pub output: Option<CliOutputFormat>,

    /// Output as JSON (shorthand for --output json)
    #[arg(short = 'j', long, global = true, conflicts_with_all = ["output", "yaml"])]
    pub json: bool,

    /// Output as YAML (shorthand for --output yaml)
    #[arg(short = 'y', long, global = true, conflicts_with_all = ["output", "json"])]
    pub yaml: bool,

    /// Verbose logging
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Show this CLI build and the server build for the selected profile.
    Info {
        /// Show only the local binary, without contacting the server.
        #[arg(long)]
        local: bool,
    },
    /// Emit shell completion setup.
    Completion {
        #[command(subcommand)]
        command: CompletionCommand,
    },
    #[command(name = "__complete", hide = true)]
    Complete {
        #[arg(long)]
        cursor: Option<usize>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        words: Vec<String>,
    },
    /// Authentication commands
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },
    /// Permission sets and their identity/role assignments
    Permission {
        #[command(subcommand)]
        command: PermissionCommands,
    },
    /// Identity inspection and manual role membership
    Identity {
        #[command(subcommand)]
        command: IdentityCommands,
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
    /// Policy management
    Policy {
        #[command(subcommand)]
        command: PolicyCommands,
    },
    /// Key/secret management
    Key {
        #[command(subcommand)]
        command: KeyCommands,
    },
    /// Versioned external data cache management
    Cache {
        #[command(subcommand)]
        command: CacheCommands,
    },
    /// Execution monitoring
    Execution {
        #[command(subcommand)]
        command: ExecutionCommands,
    },
    /// Human inquiry management
    Inquiry {
        #[command(subcommand)]
        command: InquiryCommands,
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

        #[command(flatten)]
        execution_options: ManualExecutionOptions,

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

#[derive(Subcommand, Clone, Copy)]
pub enum CompletionCommand {
    /// Emit a Bash completion script.
    Bash,
    /// Emit a Fish completion script.
    Fish,
    /// Emit a Zsh completion script.
    Zsh,
    /// Emit a PowerShell completion script.
    #[command(name = "powershell")]
    PowerShell,
    /// Install a completion script in the current user's shell directory.
    Install {
        #[arg(value_enum)]
        shell: CompletionShell,
    },
}

#[derive(ValueEnum, Clone, Copy)]
pub enum CompletionShell {
    Bash,
    Fish,
    Zsh,
}

/// Command-line output choices. NDJSON is deliberately limited to cache scans
/// that explicitly opt into streaming the complete pinned snapshot.
#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq)]
pub enum CliOutputFormat {
    Table,
    Json,
    Yaml,
    Ndjson,
}

impl From<CliOutputFormat> for crate::output::OutputFormat {
    fn from(value: CliOutputFormat) -> Self {
        match value {
            CliOutputFormat::Table => Self::Table,
            CliOutputFormat::Json => Self::Json,
            CliOutputFormat::Yaml => Self::Yaml,
            // `main` rejects this for all non-cache commands before it reaches
            // the normal output renderer.
            CliOutputFormat::Ndjson => Self::Table,
        }
    }
}
