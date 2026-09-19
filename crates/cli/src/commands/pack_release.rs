use anyhow::Result;
use attune_common::pack_format::{self, ArtifactInput, BuildOptions, Limits};
use clap::Subcommand;
use std::path::PathBuf;

use crate::cli::CliOutputFormat;
use crate::output::OutputFormat;

#[derive(Subcommand, Clone)]
pub enum ReleaseCommands {
    /// Assemble source and generated artifacts without contacting a server
    #[command(
        after_help = "Examples:\n  attune pack release build ./acme --artifact 'tool[linux/amd64/static]=dist/tool' --output dist/acme.attune-pack.tar.gz --format json"
    )]
    Build {
        pack_dir: PathBuf,
        /// ID[linux/ARCH/static]=FILE for native, or ID=FILE for a JAR
        #[arg(long, value_name = "MAPPING")]
        artifact: Vec<ArtifactInput>,
        /// Declare a helper executable by pack-relative path, never local mode bits
        #[arg(long, value_name = "PATH")]
        executable: Vec<String>,
        /// Archive destination; must not already exist
        #[arg(long = "output", value_name = "ARCHIVE")]
        archive_output: PathBuf,
        /// Result format, separate from the archive destination
        // Shadow the inherited format flag's spelling, not its typed value or ID.
        #[arg(id = "output", long = "format", value_enum)]
        format: Option<CliOutputFormat>,
    },
    /// Verify exact archive bytes, payloads and launch metadata offline
    #[command(
        after_help = "Examples:\n  attune pack release verify dist/acme.attune-pack.tar.gz --format json"
    )]
    Verify {
        archive: PathBuf,
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
    },
    /// Verify an archive and display its release manifest and measured sizes
    #[command(
        after_help = "Examples:\n  attune pack release inspect dist/acme.attune-pack.tar.gz --format json"
    )]
    Inspect {
        archive: PathBuf,
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
    },
}

pub async fn handle(command: ReleaseCommands, default_format: OutputFormat) -> Result<()> {
    let inspect = matches!(command, ReleaseCommands::Inspect { .. });
    let (release, format) = tokio::task::spawn_blocking(move || -> Result<_> {
        let limits = Limits::default();
        Ok(match command {
            ReleaseCommands::Build {
                pack_dir,
                artifact,
                executable,
                archive_output,
                format,
            } => {
                let options = BuildOptions {
                    source: pack_dir,
                    artifacts: artifact,
                    executables: executable,
                };
                (
                    pack_format::build(&options, &archive_output, limits)?,
                    format.map(OutputFormat::from),
                )
            }
            ReleaseCommands::Verify { archive, format } => {
                (pack_format::verify(&archive, limits)?, format)
            }
            ReleaseCommands::Inspect { archive, format } => {
                (pack_format::inspect(&archive, limits)?, format)
            }
        })
    })
    .await??;
    match format.unwrap_or(default_format) {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&release)?),
        OutputFormat::Yaml => print!("{}", serde_yaml_ng::to_string(&release)?),
        OutputFormat::Table => {
            println!(
                "{}@{}",
                release.manifest().pack.r#ref,
                release.manifest().pack.version
            );
            println!("sha256:{}", release.sha256);
            println!(
                "{} compressed bytes, {} payload/manifest bytes, {} files",
                release.compressed_size, release.extracted_size, release.entry_count
            );
            if inspect {
                println!("{}", serde_json::to_string_pretty(release.manifest())?);
            }
        }
    }
    Ok(())
}
