use std::io::Read;

use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use serde_json::{json, Value};

use crate::{
    client::ApiClient,
    config::CliConfig,
    output::{self, OutputFormat},
};

#[derive(Subcommand)]
pub enum SchemaTypeCommands {
    /// List installed named schemas. Authentication is sufficient.
    #[command(
        after_help = "Examples:\n  attune schema-type list --pack core --json\n  attune schema-type list --query address"
    )]
    List {
        #[arg(long)]
        pack: Option<String>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        page: u32,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100))]
        page_size: u32,
    },
    /// Show the original type definition without replacing authored references
    #[command(after_help = "Examples:\n  attune schema-type show core.address --json")]
    Show { type_ref: String },
    /// Inspect the self-contained resolved graph and selected revisions
    #[command(after_help = "Examples:\n  attune schema-type inspect core.address --json")]
    Inspect { type_ref: String },
    /// Compile a rendering preview. This never binds or mutates a runtime value.
    #[command(
        after_help = "Examples:\n  attune schema-type preview --context single-value --file schema.json --json\n  cat params.json | attune schema-type preview --context flat-document --file - --json"
    )]
    Preview {
        #[arg(long, value_enum)]
        context: PreviewContext,
        /// JSON schema file, or '-' for stdin. Input is bounded to 1 MiB.
        #[arg(long)]
        file: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum PreviewContext {
    FlatDocument,
    SingleValue,
}

pub async fn handle_schema_type_command(
    profile: &Option<String>,
    command: SchemaTypeCommands,
    api_url: &Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let value = match command {
        SchemaTypeCommands::List {
            pack,
            query,
            page,
            page_size,
        } => {
            let mut path = pack
                .map(|pack| format!("/packs/{}/schema-types", encode(&pack)))
                .unwrap_or_else(|| "/schema-types".into());
            path.push_str(&format!("?page={page}&page_size={page_size}"));
            if let Some(query) = query {
                path.push_str(&format!("&q={}", encode(&query)));
            }
            serde_json::to_value(client.get_paginated_response::<Value>(&path).await?)?
        }
        SchemaTypeCommands::Show { type_ref } => {
            client
                .get::<Value>(&format!("/schema-types/{}", encode(&type_ref)))
                .await?
        }
        SchemaTypeCommands::Inspect { type_ref } => {
            client
                .get::<Value>(&format!("/schema-types/{}/inspect", encode(&type_ref)))
                .await?
        }
        SchemaTypeCommands::Preview { context, file } => {
            let mut reader: Box<dyn Read> = if file == "-" {
                Box::new(std::io::stdin())
            } else {
                Box::new(
                    std::fs::File::open(&file)
                        .with_context(|| format!("Cannot open schema file {file}"))?,
                )
            };
            let mut bytes = Vec::new();
            reader
                .by_ref()
                .take((1 << 20) + 1)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(bytes.len() <= 1 << 20, "Schema input exceeds 1 MiB");
            let schema: Value =
                serde_json::from_slice(&bytes).context("Expected a JSON schema document")?;
            let context = match context {
                PreviewContext::FlatDocument => "flat_document",
                PreviewContext::SingleValue => "single_value",
            };
            client
                .post::<Value, _>(
                    "/schema-types/preview",
                    &json!({"context": context, "schema": schema}),
                )
                .await?
        }
    };
    // Graphs cannot be flattened into a table without losing their meaning.
    output::print_output(
        &value,
        if matches!(format, OutputFormat::Table) {
            OutputFormat::Json
        } else {
            format
        },
    )
}

fn encode(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: SchemaTypeCommands,
    }

    #[test]
    fn discovery_and_stdin_preview_are_noninteractive_and_bounded() {
        assert!(TestCli::try_parse_from([
            "schema-type",
            "preview",
            "--context",
            "single-value",
            "--file",
            "-"
        ])
        .is_ok());
        assert!(TestCli::try_parse_from(["schema-type", "preview", "--file", "-"]).is_err());
        assert!(TestCli::try_parse_from(["schema-type", "list", "--page-size", "101"]).is_err());
        assert!(TestCli::try_parse_from(["schema-type", "list", "--page", "0"]).is_err());
        assert_eq!(encode("pack/a?query"), "pack%2Fa%3Fquery");
    }
}
