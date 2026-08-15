use anyhow::Result;
use serde::Deserialize;
use std::{env, time::Duration};

use crate::{client::ApiClient, config::CliConfig};

const COMPLETION_API_TIMEOUT: Duration = Duration::from_secs(2);
const ROOT_COMMANDS: &[&str] = &[
    "auth",
    "pack",
    "action",
    "rule",
    "queue",
    "policy",
    "key",
    "cache",
    "execution",
    "workflow",
    "trigger",
    "sensor",
    "artifact",
    "audit",
    "config",
    "run",
    "completion",
];
const GLOBAL_OPTIONS: &[&str] = &[
    "--profile",
    "--api-url",
    "--output",
    "--json",
    "--yaml",
    "--verbose",
    "--help",
    "--version",
];
const EXECUTION_OPTIONS: &[&str] = &[
    "--param",
    "--params-json",
    "--worker-selector",
    "--worker-tolerations",
    "--worker-affinity",
    "--execution-timeout",
    "--watch",
    "--timeout",
    "--notifier-url",
];
const VALUE_OPTIONS: &[&str] = &[
    "--profile",
    "--api-url",
    "--output",
    "--param",
    "--params-json",
    "--worker-selector",
    "--worker-tolerations",
    "--worker-affinity",
    "--execution-timeout",
    "--timeout",
    "--notifier-url",
];

#[derive(Debug, Deserialize)]
struct ActionSearchHit {
    #[serde(rename = "ref")]
    action_ref: String,
}
#[derive(Debug, Deserialize)]
struct PackSummary {
    #[serde(rename = "ref")]
    pack_ref: String,
}
#[derive(Debug, Deserialize)]
struct ActionDetail {
    param_schema: Option<serde_json::Value>,
}

pub fn bash_completion_script() -> &'static str {
    r#"_attune() {
    local cur previous command_line prefix parameter_name
    cur="${COMP_WORDS[COMP_CWORD]}"
    previous="${COMP_WORDS[COMP_CWORD-1]}"
    command_line=" ${COMP_WORDS[*]:1} "
    mapfile -t COMPREPLY < <(attune __complete --cursor "$((COMP_CWORD - 1))" "${COMP_WORDS[@]:1}")
    if [[ "$cur" == --param=*=* ]]; then
        parameter_name="${cur#--param=}"; parameter_name="${parameter_name%%=*}"
        prefix="--param=${parameter_name}="
        mapfile -t COMPREPLY < <(compgen -f -- "${cur#*=*=}")
        COMPREPLY=( "${COMPREPLY[@]/#/$prefix}" )
    elif [[ "$previous" == --file || "$previous" == --input || "$previous" == --external-id-file || "$previous" == --index ]]; then
        mapfile -t COMPREPLY < <(compgen -f -- "$cur")
    fi
    [[ "$cur" == *'.' || "$cur" == *'=' ]] && compopt -o nospace
}
complete -F _attune attune
"#
}

pub fn zsh_completion_script() -> &'static str {
    r#"#compdef attune
_attune() {
  local -a candidates
  local current="${words[CURRENT]}"
  candidates=("${(@f)$(attune __complete --cursor "$((CURRENT - 1))" "${words[@]:1}")}")
  (( ${#candidates} )) && compadd -S '' -a candidates
  [[ "$current" == --file=* || "$current" == --input=* ]] && _files
}
_attune "$@"
"#
}

pub fn fish_completion_script() -> &'static str {
    r#"function __attune_dynamic_complete
    set -l words (commandline -opc)
    set -l current (commandline -ct)
    set -e words[1]
    # Preserve the empty token after `attune run ` so the helper recognizes
    # that it is completing the action reference rather than a global option.
    set -a words "$current"
    set -l cursor (math (count $words) - 1)
    attune __complete --cursor $cursor $words
end
function __attune_no_path_context
    return 0
end
complete -c attune -f -a '(__attune_dynamic_complete)'
complete -c attune -n '__attune_no_path_context' -f
"#
}

/// Print candidates without letting API or config failures affect the shell.
pub async fn print_candidates(words: &[String], cursor: usize) {
    if let Ok(Ok(candidates)) =
        tokio::time::timeout(COMPLETION_API_TIMEOUT, candidates(words, cursor)).await
    {
        for candidate in candidates {
            println!("{candidate}");
        }
    }
}

async fn candidates(words: &[String], cursor: usize) -> Result<Vec<String>> {
    let cursor = cursor.min(words.len());
    let words = &words[..cursor.saturating_add(1).min(words.len())];
    let current = words.last().map(String::as_str).unwrap_or_default();
    if let Some(prefix) = profile_completion_prefix(words) {
        return complete_profiles(prefix);
    }
    let Some((action_index, action_ref)) = execution_context(words) else {
        return Ok(static_candidates(words, current));
    };
    if action_ref.is_empty() {
        return complete_packs(words).await;
    }
    if words.len() == action_index + 1 {
        return complete_actions(action_ref, words).await;
    }
    let after_action = &words[action_index + 1..];
    if let Some(parameter) = parameter_completion(after_action) {
        let mut values = complete_parameter(action_ref, parameter.value, words).await?;
        if parameter.attached {
            values = values
                .into_iter()
                .map(|value| format!("--param={value}"))
                .collect();
        }
        return Ok(values);
    }
    if previous_requires_value(after_action) {
        return Ok(Vec::new());
    }
    if current.starts_with('-') || current.is_empty() {
        return Ok(EXECUTION_OPTIONS
            .iter()
            .chain(GLOBAL_OPTIONS)
            .filter(|option| option.starts_with(current))
            .map(|option| (*option).to_string())
            .collect());
    }
    Ok(Vec::new())
}

fn static_candidates(words: &[String], current: &str) -> Vec<String> {
    let non_options = positional_words(words);
    match non_options.as_slice() {
        [(_, "action")] => [
            "list", "get", "create", "update", "enable", "disable", "delete", "execute",
        ]
        .iter()
        .filter(|item| item.starts_with(current))
        .map(|item| (*item).to_string())
        .collect(),
        [] | [_] => ROOT_COMMANDS
            .iter()
            .chain(GLOBAL_OPTIONS)
            .filter(|item| item.starts_with(current))
            .map(|item| (*item).to_string())
            .collect(),
        _ => GLOBAL_OPTIONS
            .iter()
            .filter(|item| item.starts_with(current))
            .map(|item| (*item).to_string())
            .collect(),
    }
}

fn positional_words(words: &[String]) -> Vec<(usize, &str)> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let word = &words[index];
        if VALUE_OPTIONS.contains(&word.as_str()) || word == "-p" {
            index += 2;
            continue;
        }
        if word.starts_with("--") || word.starts_with("-p") {
            index += 1;
            continue;
        }
        result.push((index, word.as_str()));
        index += 1;
    }
    result
}

struct Parameter<'a> {
    value: &'a str,
    attached: bool,
}
fn parameter_completion(words: &[String]) -> Option<Parameter<'_>> {
    let current = words.last()?.as_str();
    if current == "--param" {
        return Some(Parameter {
            value: "",
            attached: false,
        });
    }
    if let Some(value) = current.strip_prefix("--param=") {
        return Some(Parameter {
            value,
            attached: true,
        });
    }
    if words.len() >= 2 && words[words.len() - 2] == "--param" {
        return Some(Parameter {
            value: current,
            attached: false,
        });
    }
    None
}
fn previous_requires_value(words: &[String]) -> bool {
    words
        .last()
        .is_some_and(|word| VALUE_OPTIONS.contains(&word.as_str()))
}
fn profile_completion_prefix(words: &[String]) -> Option<&str> {
    let current = words.last()?.as_str();
    if current == "--profile" || current == "-p" {
        return Some("");
    }
    if let Some(prefix) = current.strip_prefix("--profile=") {
        return Some(prefix);
    }
    (words.len() >= 2 && matches!(words[words.len() - 2].as_str(), "--profile" | "-p"))
        .then_some(current)
}
fn complete_profiles(prefix: &str) -> Result<Vec<String>> {
    let Some(config) = CliConfig::load_existing()? else {
        return Ok(Vec::new());
    };
    let mut profiles: Vec<_> = config
        .profiles
        .into_keys()
        .filter(|profile| profile.starts_with(prefix))
        .collect();
    profiles.sort();
    Ok(profiles)
}
fn execution_context(words: &[String]) -> Option<(usize, &str)> {
    let positionals = positional_words(words);
    let action_position = match positionals.as_slice() {
        [(_, "run"), rest @ ..] => rest.first().copied(),
        [(_, "action"), (_, "execute"), rest @ ..] => rest.first().copied(),
        _ => return None,
    };
    Some(action_position.unwrap_or((words.len(), "")))
}
async fn complete_actions(prefix: &str, words: &[String]) -> Result<Vec<String>> {
    let (config, api_url) = completion_config(words)?;
    let mut client = ApiClient::from_config_with_timeout(&config, &api_url, COMPLETION_API_TIMEOUT);
    let actions: Vec<ActionSearchHit> = client
        .get_paginated(&format!(
            "/actions/search?q={}&page_size=100",
            urlencoding::encode(prefix)
        ))
        .await?;
    Ok(actions
        .into_iter()
        .map(|action| action.action_ref)
        .filter(|reference| reference.starts_with(prefix))
        .collect())
}
async fn complete_packs(words: &[String]) -> Result<Vec<String>> {
    let (config, api_url) = completion_config(words)?;
    let mut client = ApiClient::from_config_with_timeout(&config, &api_url, COMPLETION_API_TIMEOUT);
    let packs: Vec<PackSummary> = client.get_paginated("/packs?page_size=100").await?;
    Ok(packs
        .into_iter()
        .map(|pack| format!("{}.", pack.pack_ref))
        .collect())
}
async fn complete_parameter(
    action_ref: &str,
    current: &str,
    words: &[String],
) -> Result<Vec<String>> {
    let (config, api_url) = completion_config(words)?;
    let mut client = ApiClient::from_config_with_timeout(&config, &api_url, COMPLETION_API_TIMEOUT);
    let action: ActionDetail = client
        .get(&format!("/actions/{}", urlencoding::encode(action_ref)))
        .await?;
    let Some(schema) = action
        .param_schema
        .and_then(|schema| schema.as_object().cloned())
    else {
        return Ok(Vec::new());
    };
    if let Some((name, value_prefix)) = current.split_once('=') {
        return Ok(schema
            .get(name)
            .and_then(|field| field.get("enum"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| match value {
                serde_json::Value::String(value) => Some(value.clone()),
                serde_json::Value::Number(_) | serde_json::Value::Bool(_) => {
                    Some(value.to_string())
                }
                _ => None,
            })
            .filter(|value| value.starts_with(value_prefix))
            .map(|value| format!("{name}={value}"))
            .collect());
    }
    Ok(schema
        .into_iter()
        .map(|(name, _)| name)
        .map(|name| format!("{name}="))
        .filter(|candidate| candidate.starts_with(current))
        .collect())
}
fn completion_config(words: &[String]) -> Result<(CliConfig, Option<String>)> {
    let mut profile = env::var("ATTUNE_PROFILE").ok();
    let mut api_url = env::var("ATTUNE_API_URL").ok();
    let mut index = 0;
    while index < words.len() {
        match words[index].as_str() {
            "--profile" | "-p" => {
                profile = words.get(index + 1).cloned();
                index += 1;
            }
            value if value.starts_with("--profile=") => {
                profile = value.strip_prefix("--profile=").map(str::to_owned)
            }
            "--api-url" => {
                api_url = words.get(index + 1).cloned();
                index += 1;
            }
            value if value.starts_with("--api-url=") => {
                api_url = value.strip_prefix("--api-url=").map(str::to_owned)
            }
            _ => {}
        }
        index += 1;
    }
    let config = CliConfig::load_existing_with_profile(profile.as_deref())?
        .ok_or_else(|| anyhow::anyhow!("Attune configuration does not exist"))?;
    Ok((config, api_url))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_option_ordering_and_attached_values() {
        let words = vec![
            "run".into(),
            "--watch".into(),
            "core.echo".into(),
            "--param=mode=f".into(),
        ];
        assert_eq!(execution_context(&words), Some((2, "core.echo")));
        assert!(parameter_completion(&words[3..]).is_some());
    }
    #[test]
    fn ignores_option_values_as_commands() {
        let words = vec![
            "--profile".into(),
            "run".into(),
            "action".into(),
            "execute".into(),
            "core.echo".into(),
        ];
        assert_eq!(execution_context(&words), Some((4, "core.echo")));
    }
}
