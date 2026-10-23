use anyhow::Result;
use clap::ValueEnum;
use colored::Colorize;
use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Cell, Color, ContentArrangement, Table,
};
use serde::Serialize;
use std::fmt::Display;
use terminal_size::{terminal_size, Width};

/// Minimum width to use for table layout, even if the terminal is narrower.
const MIN_TABLE_WIDTH: u16 = 60;

/// A command has already emitted its complete report to stdout. Returning this
/// error preserves a failing exit status without appending a second document.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ReportedFailure(pub String);

/// Output format for CLI commands
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq)]
pub enum OutputFormat {
    /// Human-readable table format
    Table,
    /// JSON format for scripting
    Json,
    /// YAML format
    Yaml,
}

impl Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::Table => write!(f, "table"),
            OutputFormat::Json => write!(f, "json"),
            OutputFormat::Yaml => write!(f, "yaml"),
        }
    }
}

/// Print output in the specified format
pub fn print_output<T: Serialize>(data: &T, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(data)?;
            println!("{}", json);
        }
        OutputFormat::Yaml => {
            let yaml = serde_yaml_ng::to_string(data)?;
            println!("{}", yaml);
        }
        OutputFormat::Table => {
            // For table format, the caller should use specific table functions
            let json = serde_json::to_string_pretty(data)?;
            println!("{}", json);
        }
    }
    Ok(())
}

/// Print a success message
pub fn print_success(message: &str) {
    println!("{} {}", "✓".green().bold(), message);
}

/// Print an info message
pub fn print_info(message: &str) {
    println!("{} {}", "ℹ".blue().bold(), message);
}

/// Print a warning message
pub fn print_warning(message: &str) {
    eprintln!("{} {}", "⚠".yellow().bold(), message);
}

/// Print an error message
pub fn print_error(message: &str) {
    eprintln!("{} {}", "✗".red().bold(), message);
}

/// Create a new table with default styling.
///
/// The table is configured to wrap cell content dynamically to fit the
/// terminal width, falling back to a sensible minimum width when the
/// terminal is very narrow or its size cannot be detected.
pub fn create_table() -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_width(table_width());
    table
}

/// Width to use for table layout.
///
/// When attached to a terminal, the detected width is used (clamped to a
/// sensible minimum). When there is no terminal (non-interactive use such as
/// CI, piped output, or test harnesses), we deliberately return a very large
/// value so dynamic content arrangement does not wrap cells — wrapping in
/// non-interactive output makes downstream parsing brittle.
fn table_width() -> u16 {
    match terminal_size() {
        Some((Width(w), _)) => w.max(MIN_TABLE_WIDTH),
        None => u16::MAX,
    }
}

/// Add a header row to a table with styling
pub fn add_header(table: &mut Table, headers: Vec<&str>) {
    let cells: Vec<Cell> = headers
        .into_iter()
        .map(|h| Cell::new(h).fg(Color::Cyan))
        .collect();
    table.set_header(cells);
}

/// Print a table of key-value pairs
pub fn print_key_value_table(pairs: Vec<(&str, String)>) {
    let mut table = create_table();
    add_header(&mut table, vec!["Key", "Value"]);
    let width = terminal_width();
    let key_width = pairs
        .iter()
        .map(|(key, _)| display_width(key))
        .max()
        .unwrap_or(3)
        .clamp(8, 18);
    let value_width = width.saturating_sub(key_width + 9).max(20);

    for (key, value) in pairs {
        table.add_row(vec![
            Cell::new(wrap_text(key, key_width)).fg(Color::Yellow),
            Cell::new(wrap_text(&value, value_width)),
        ]);
    }

    println!("{}", table);
}

/// Print a schema in a readable multi-line format instead of a raw JSON dump.
pub fn print_schema(schema: &serde_json::Value) -> Result<()> {
    if let Some(properties) = schema.as_object() {
        if properties.values().all(|value| value.is_object()) {
            let width = terminal_width();
            let content_width = width.saturating_sub(4).max(24);
            let mut names = properties.keys().collect::<Vec<_>>();
            names.sort();

            for (index, name) in names.into_iter().enumerate() {
                if index > 0 {
                    println!();
                }

                println!("{}", name.bold());
                if let Some(definition) = properties.get(name).and_then(|value| value.as_object()) {
                    print_schema_field("Type", &schema_type_label(definition), content_width);

                    if let Some(default) = definition.get("default") {
                        print_schema_field("Default", &compact_json(default), content_width);
                    }

                    if let Some(description) = definition
                        .get("description")
                        .and_then(|value| value.as_str())
                    {
                        print_schema_field("Description", description, content_width);
                    }

                    let constraints = schema_constraints(definition);
                    if !constraints.is_empty() {
                        print_schema_field("Constraints", &constraints.join(", "), content_width);
                    }
                }
            }

            return Ok(());
        }
    }

    println!("{}", serde_yaml_ng::to_string(schema)?);
    Ok(())
}

/// Print a simple list
pub fn print_list(items: Vec<String>) {
    for item in items {
        println!("  • {}", item);
    }
}

/// Print a titled section
pub fn print_section(title: &str) {
    println!("\n{}", title.bold().underline());
}

/// Format a boolean as a colored checkmark or cross
pub fn format_bool(value: bool) -> String {
    if value {
        "✓".green().to_string()
    } else {
        "✗".red().to_string()
    }
}

/// Format a status with color
pub fn format_status(status: &str) -> String {
    match status.to_lowercase().as_str() {
        "succeeded" | "success" | "enabled" | "active" | "running" => status.green().to_string(),
        "failed" | "error" | "disabled" | "inactive" => status.red().to_string(),
        "pending" | "scheduled" | "queued" => status.yellow().to_string(),
        "canceled" | "cancelled" => status.bright_black().to_string(),
        _ => status.to_string(),
    }
}

/// Truncate a string to a maximum length with ellipsis
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

fn terminal_width() -> usize {
    terminal_size()
        .map(|(Width(width), _)| width as usize)
        .filter(|width| *width > 20)
        .unwrap_or(100)
}

fn display_width(value: &str) -> usize {
    value.chars().count()
}

fn wrap_text(value: &str, width: usize) -> String {
    let width = width.max(1);
    let mut wrapped = Vec::new();

    for paragraph in value.split('\n') {
        if paragraph.is_empty() {
            wrapped.push(String::new());
            continue;
        }

        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if line.is_empty() {
                append_wrapped_word(&mut wrapped, &mut line, word, width);
                continue;
            }

            if display_width(&line) + 1 + display_width(word) <= width {
                line.push(' ');
                line.push_str(word);
            } else {
                wrapped.push(line);
                line = String::new();
                append_wrapped_word(&mut wrapped, &mut line, word, width);
            }
        }

        if !line.is_empty() {
            wrapped.push(line);
        }
    }

    wrapped.join("\n")
}

fn append_wrapped_word(
    lines: &mut Vec<String>,
    current_line: &mut String,
    word: &str,
    width: usize,
) {
    if display_width(word) <= width {
        current_line.push_str(word);
        return;
    }

    let mut chunk = String::new();
    for ch in word.chars() {
        chunk.push(ch);
        if display_width(&chunk) >= width {
            if current_line.is_empty() {
                lines.push(std::mem::take(&mut chunk));
            } else {
                lines.push(std::mem::take(current_line));
                lines.push(std::mem::take(&mut chunk));
            }
        }
    }

    if !chunk.is_empty() {
        current_line.push_str(&chunk);
    }
}

fn schema_type_label(definition: &serde_json::Map<String, serde_json::Value>) -> String {
    match definition.get("type") {
        Some(serde_json::Value::String(kind)) => kind.clone(),
        Some(serde_json::Value::Array(kinds)) => kinds
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>()
            .join(" | "),
        _ => "any".to_string(),
    }
}

fn schema_constraints(definition: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    let mut constraints = Vec::new();

    if let Some(values) = definition.get("enum").and_then(|value| value.as_array()) {
        constraints.push(format!(
            "enum: {}",
            values
                .iter()
                .map(compact_json)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    for key in [
        "minimum",
        "maximum",
        "minLength",
        "maxLength",
        "pattern",
        "format",
    ] {
        if let Some(value) = definition.get(key) {
            constraints.push(format!("{key}: {}", compact_json(value)));
        }
    }

    constraints
}

fn compact_json(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

fn print_schema_field(label: &str, value: &str, width: usize) {
    let indent = "  ";
    let label_prefix = format!("{indent}{label}: ");
    let continuation = " ".repeat(label_prefix.chars().count());
    let wrapped = wrap_text(
        value,
        width.saturating_sub(label_prefix.chars().count()).max(12),
    );
    let mut lines = wrapped.lines();

    if let Some(first_line) = lines.next() {
        println!("{label_prefix}{first_line}");
    }

    for line in lines {
        println!("{continuation}{line}");
    }
}

/// Format a timestamp in a human-readable way
pub fn format_timestamp(timestamp: &str) -> String {
    // Try to parse and format nicely, otherwise return as-is
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        dt.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        timestamp.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("this is a long string", 10), "this is...");
        assert_eq!(truncate("exactly10!", 10), "exactly10!");
    }

    #[test]
    fn test_output_format_display() {
        assert_eq!(OutputFormat::Table.to_string(), "table");
        assert_eq!(OutputFormat::Json.to_string(), "json");
        assert_eq!(OutputFormat::Yaml.to_string(), "yaml");
    }
}
