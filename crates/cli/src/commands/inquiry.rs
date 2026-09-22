use std::io::{self, Read};

use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use serde_json::Value;

use crate::client::{ApiClient, PaginatedResponse};
use crate::config::CliConfig;
use crate::inquiry::{self, Inquiry, InquiryListFilters, InquirySummary};
use crate::output::{self, OutputFormat};

#[derive(Subcommand)]
pub enum InquiryCommands {
    /// List visible inquiries
    List {
        /// Filter by inquiry status
        #[arg(long, value_enum)]
        status: Option<InquiryStatus>,

        /// Filter by the execution that created the inquiry
        #[arg(long)]
        created_by_execution: Option<i64>,

        /// Filter by assigned identity ID
        #[arg(long)]
        assigned_to: Option<i64>,

        /// Number of matching inquiries to skip
        #[arg(long, default_value_t = 0)]
        offset: usize,

        /// Maximum number of inquiries to return
        #[arg(long, default_value_t = 50, value_parser = parse_limit)]
        limit: usize,
    },
    /// Show one inquiry
    Show {
        /// Inquiry ID
        inquiry_id: i64,
    },
    /// Respond to a pending inquiry
    Respond {
        /// Inquiry ID
        inquiry_id: i64,

        /// Use a stored response option by ref
        #[arg(
            long,
            required_unless_present = "response_json",
            conflicts_with = "response_json"
        )]
        option: Option<String>,

        /// Submit a JSON response object
        #[arg(long, required_unless_present = "option", conflicts_with = "option")]
        response_json: Option<String>,
    },
    /// Operations that require the creator execution's execution token
    Execution {
        #[command(subcommand)]
        command: ExecutionInquiryCommands,
    },
}

#[derive(Subcommand)]
pub enum ExecutionInquiryCommands {
    /// Create an inquiry using an execution token
    Create {
        /// JSON request file, or '-' to read from stdin
        #[arg(long)]
        request_file: String,
    },
    /// Cancel an inquiry created by the current execution token
    Cancel {
        /// Inquiry ID
        inquiry_id: i64,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum InquiryStatus {
    Pending,
    Responded,
    Timeout,
    Cancelled,
}

impl InquiryStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Responded => "responded",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
        }
    }
}

fn parse_limit(value: &str) -> std::result::Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| "limit must be an integer".to_string())?;
    if (1..=500).contains(&limit) {
        Ok(limit)
    } else {
        Err("limit must be between 1 and 500".to_string())
    }
}

pub async fn handle_inquiry_command(
    profile: &Option<String>,
    command: InquiryCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    match command {
        InquiryCommands::List {
            status,
            created_by_execution,
            assigned_to,
            offset,
            limit,
        } => {
            let page = inquiry::list(
                &mut client,
                &InquiryListFilters {
                    status: status.map(|value| value.as_str().to_string()),
                    created_by_execution,
                    assigned_to,
                    offset,
                    limit,
                },
            )
            .await?;
            print_inquiry_list(&page, output_format)
        }
        InquiryCommands::Show { inquiry_id } => {
            let inquiry = inquiry::get(&mut client, inquiry_id).await?;
            print_inquiry(&inquiry, output_format)
        }
        InquiryCommands::Respond {
            inquiry_id,
            option,
            response_json,
        } => {
            let inquiry = if let Some(option_ref) = option {
                inquiry::respond_with_option(&mut client, inquiry_id, &option_ref).await?
            } else {
                let response = serde_json::from_str(
                    response_json
                        .as_deref()
                        .expect("clap requires --option or --response-json"),
                )
                .context("--response-json must contain valid JSON")?;
                inquiry::respond(&mut client, inquiry_id, response).await?
            };
            print_inquiry(&inquiry, output_format)
        }
        InquiryCommands::Execution { command } => match command {
            ExecutionInquiryCommands::Create { request_file } => {
                let request = read_request(&request_file)?;
                let response = inquiry::create(&mut client, request).await?;
                output::print_output(&response, output_format)
            }
            ExecutionInquiryCommands::Cancel { inquiry_id } => {
                let inquiry = inquiry::cancel(&mut client, inquiry_id).await?;
                print_inquiry(&inquiry, output_format)
            }
        },
    }
}

fn read_request(path: &str) -> Result<Value> {
    let content = if path == "-" {
        let mut content = String::new();
        io::stdin()
            .read_to_string(&mut content)
            .context("Failed to read inquiry request from stdin")?;
        content
    } else {
        std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read inquiry request file '{path}'"))?
    };
    serde_json::from_str(&content).context("Inquiry request must contain valid JSON")
}

fn print_inquiry_list(
    page: &PaginatedResponse<InquirySummary>,
    format: OutputFormat,
) -> Result<()> {
    if format != OutputFormat::Table {
        return output::print_output(page, format);
    }

    let mut table = output::create_table();
    output::add_header(
        &mut table,
        vec![
            "ID",
            "Status",
            "Prompt",
            "Created by",
            "Assigned to",
            "Created",
        ],
    );
    for inquiry in &page.items {
        table.add_row(vec![
            inquiry.id.to_string(),
            output::format_status(&inquiry.status),
            output::truncate(&inquiry.prompt, 60),
            inquiry.created_by_execution.to_string(),
            inquiry
                .assigned_to
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            output::format_timestamp(&inquiry.created),
        ]);
    }
    println!("{table}");
    Ok(())
}

fn print_inquiry(inquiry: &Inquiry, format: OutputFormat) -> Result<()> {
    if format != OutputFormat::Table {
        return output::print_output(inquiry, format);
    }

    output::print_key_value_table(vec![
        ("ID", inquiry.id.to_string()),
        ("Status", inquiry.status.clone()),
        ("Prompt", inquiry.prompt.clone()),
        (
            "Purpose",
            inquiry.purpose.clone().unwrap_or_else(|| "-".to_string()),
        ),
        ("Created by", inquiry.created_by_execution.to_string()),
        (
            "Assigned to",
            inquiry
                .assigned_to
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "Response schema",
            inquiry
                .response_schema
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "Response options",
            serde_json::to_string(&inquiry.response_options)?,
        ),
        (
            "Response",
            inquiry
                .response
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("Created", output::format_timestamp(&inquiry.created)),
        ("Updated", output::format_timestamp(&inquiry.updated)),
    ]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::cli::{Cli, Commands};

    #[test]
    fn respond_requires_exactly_one_response_source() {
        assert!(Cli::try_parse_from(["attune", "inquiry", "respond", "42"]).is_err());
        assert!(Cli::try_parse_from([
            "attune",
            "inquiry",
            "respond",
            "42",
            "--option",
            "approve",
            "--response-json",
            "{}"
        ])
        .is_err());
        assert!(
            Cli::try_parse_from(["attune", "inquiry", "respond", "42", "--option", "approve"])
                .is_ok()
        );
    }

    #[test]
    fn create_and_cancel_are_nested_under_execution_authority() {
        let cli = Cli::try_parse_from([
            "attune",
            "inquiry",
            "execution",
            "create",
            "--request-file",
            "-",
        ])
        .expect("execution inquiry create should parse");
        assert!(matches!(cli.command, Commands::Inquiry { .. }));
        assert!(Cli::try_parse_from(["attune", "inquiry", "cancel", "42"]).is_err());
    }
}
