//! Prototype: clap-tree-walk static completion (ticket #5).
//!
//! Replaces the hardcoded `ROOT_COMMANDS` / `GLOBAL_OPTIONS` / action-list
//! mirrors in `completion.rs` with a walk over a `clap::Command` tree. The
//! tree is the single source of truth, so subcommand-of-subcommand completion
//! (pack, workflow, execution, trigger, sensor, rule, ...) and option lists
//! can never drift from the derive enums.
//!
//! Not yet wired into `candidates()`. The `Cli` / `Commands` enums now live in
//! the lib (`crates/cli/src/cli.rs`) so this module can build the tree via
//! `<Cli as clap::CommandFactory>::command()`.

use clap::{Arg, Command, CommandFactory};

use crate::cli::Cli;

/// Static candidates from the real CLI tree (wrapper for tests / wiring).
pub fn real_tree_candidates(words: &[String]) -> Vec<String> {
    tree_candidates(&Cli::command(), words)
}

/// Collect candidate subcommands and options at the node reached by walking
/// `words` (which includes the trailing, possibly-empty current token).
///
/// If a positional token matches no subcommand at its depth, the walk yields
/// no candidates: the token is either a value clap expects as a positional
/// (completion for those is a separate concern) or a typo, and guessing
/// siblings would be noise.
pub fn tree_candidates(root: &Command, words: &[String]) -> Vec<String> {
    let current = words.last().map(String::as_str).unwrap_or_default();
    let tokens = &words[..words.len().saturating_sub(1)];

    let mut node = root;
    let mut globals: Vec<&Arg> = Vec::new();
    for arg in node.get_arguments() {
        if arg.is_global_set() {
            globals.push(arg);
        }
    }

    let mut valid = true;
    let mut positionals_filled: usize = 0;
    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        if token.starts_with('-') {
            let matched = node
                .get_arguments()
                .chain(globals.iter().copied())
                .find(|arg| arg_matches_token(arg, token));
            if let Some(arg) = matched {
                if arg_takes_value(arg) && !token.contains('=') && i + 1 < tokens.len() {
                    i += 1;
                }
            }
            i += 1;
            continue;
        }
        if let Some(sub) = node.get_subcommands().find(|s| s.get_name() == token) {
            node = sub;
            positionals_filled = 0;
            for arg in node.get_arguments() {
                if arg.is_global_set() {
                    globals.push(arg);
                }
            }
        } else if consumes_positional(node, positionals_filled) {
            positionals_filled += 1;
        } else {
            valid = false;
        }
        i += 1;
    }

    if !valid {
        return Vec::new();
    }

    let mut out: Vec<String> = Vec::new();
    for sub in node.get_subcommands() {
        if !sub.is_hide_set() && sub.get_name().starts_with(current) {
            out.push(sub.get_name().to_string());
        }
    }
    let mut flags: Vec<String> = Vec::new();
    for arg in node
        .get_arguments()
        .chain(globals.iter().copied())
        .filter(|arg| !arg.is_hide_set())
    {
        if current.is_empty() || current.starts_with('-') {
            if let Some(long) = arg.get_long() {
                let flag = format!("--{long}");
                if flag.starts_with(current) {
                    flags.push(flag);
                }
            }
            if let Some(short) = arg.get_short() {
                let flag = format!("-{short}");
                if flag.starts_with(current) {
                    flags.push(flag);
                }
            }
        }
    }
    if node.get_name() == root.get_name() {
        // Help and version are synthesized by clap, not present in get_arguments().
        for flag in ["--help", "-h", "--version", "-V"] {
            if flag.starts_with(current) {
                flags.push(flag.to_string());
            }
        }
    }
    out.extend(flags);
    out.sort();
    out.dedup();
    out
}

fn arg_matches_token(arg: &Arg, token: &str) -> bool {
    if let Some(long) = arg.get_long() {
        if token == format!("--{long}") || token.starts_with(&format!("--{long}=")) {
            return true;
        }
    }
    if let Some(short) = arg.get_short() {
        if token == format!("-{short}") {
            return true;
        }
    }
    false
}

fn arg_takes_value(arg: &Arg) -> bool {
    arg.get_action().takes_values()
}

/// Whether a positional token can be accepted by `node` given how many
/// positional values are already filled at this level. A declared positional
/// with remaining capacity accepts the token; a node with no declared
/// positionals (pure subcommand group) does not.
fn consumes_positional(node: &Command, filled: usize) -> bool {
    let capacity: usize = node
        .get_positionals()
        .map(|arg| {
            arg.get_num_args()
                .map(|range| range.max_values())
                // No explicit num_args: a plain positional takes one value.
                .unwrap_or(1)
        })
        .sum();
    filled < capacity
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{ArgAction, Command};

    fn profile() -> Command {
        Command::new("attune")
            .arg(Arg::new("profile").long("profile").short('p').global(true))
            .arg(Arg::new("api_url").long("api-url").global(true))
            .arg(Arg::new("output").long("output").global(true))
            .arg(
                Arg::new("json")
                    .long("json")
                    .short('j')
                    .global(true)
                    .action(ArgAction::SetTrue),
            )
            .arg(
                Arg::new("verbose")
                    .short('v')
                    .long("verbose")
                    .global(true)
                    .action(ArgAction::SetTrue),
            )
            .subcommand(
                Command::new("pack")
                    .subcommand(Command::new("create"))
                    .subcommand(Command::new("list"))
                    .subcommand(Command::new("show"))
                    .subcommand(
                        Command::new("index")
                            .subcommand(Command::new("add"))
                            .subcommand(Command::new("remove")),
                    )
                    .arg(Arg::new("pack_version").long("pack-version")),
            )
            .subcommand(
                Command::new("action")
                    .subcommand(Command::new("list"))
                    .subcommand(Command::new("get"))
                    .subcommand(Command::new("execute"))
                    .subcommand(Command::new("__complete").hide(true)),
            )
            .subcommand(
                Command::new("run")
                    .arg(Arg::new("action_ref").required(true))
                    .arg(Arg::new("param").long("param").num_args(1))
                    .arg(Arg::new("watch").long("watch").action(ArgAction::SetTrue)),
            )
    }

    fn cands(tree: &Command, words: &[&str]) -> Vec<String> {
        tree_candidates(
            tree,
            &words.iter().map(|w| w.to_string()).collect::<Vec<_>>(),
        )
    }

    #[test]
    fn root_suggests_subcommands_and_global_options() {
        let tree = profile();
        let out = cands(&tree, &[""]);
        assert!(out.contains(&"pack".to_string()));
        assert!(out.contains(&"action".to_string()));
        assert!(out.contains(&"--profile".to_string()));
        assert!(out.contains(&"--api-url".to_string()));
        assert!(out.contains(&"--help".to_string()));
        // hidden __complete must not surface
        assert!(!out.contains(&"__complete".to_string()));
    }

    #[test]
    fn root_prefix_filters() {
        let tree = profile();
        let out = cands(&tree, &["p"]);
        assert_eq!(out, vec!["pack".to_string()]);
    }

    #[test]
    fn pack_suggests_subcommands() {
        let tree = profile();
        let out = cands(&tree, &["pack", ""]);
        assert!(out.contains(&"create".to_string()));
        assert!(out.contains(&"list".to_string()));
        assert!(out.contains(&"index".to_string()));
        // local option of pack subcommand
        assert!(out.contains(&"--pack-version".to_string()));
        // global options inherited from root
        assert!(out.contains(&"--profile".to_string()));
    }

    #[test]
    fn pack_index_suggests_nested_subcommands() {
        let tree = profile();
        let out = cands(&tree, &["pack", "index", ""]);
        assert!(out.contains(&"add".to_string()));
        assert!(out.contains(&"remove".to_string()));
        assert!(!out.contains(&"create".to_string()));
    }

    #[test]
    fn action_dead_branch_is_alive_in_tree() {
        let tree = profile();
        let out = cands(&tree, &["action", ""]);
        assert!(out.contains(&"list".to_string()));
        assert!(out.contains(&"execute".to_string()));
        assert!(!out.contains(&"__complete".to_string()));
    }

    #[test]
    fn run_option_values_are_skipped() {
        let tree = profile();
        // --param takes a value: the value must not be treated as a subcommand,
        // and after the value we should still be at the run node.
        let out = cands(&tree, &["run", "core.echo", "--param", ""]);
        assert!(out.contains(&"--watch".to_string()));
        assert!(out.contains(&"--param".to_string()));
        assert!(out.contains(&"--profile".to_string()));
    }

    #[test]
    fn option_prefix_suggests_matching_flags_only() {
        let tree = profile();
        let out = cands(&tree, &["--v"]);
        assert_eq!(out, vec!["--verbose".to_string(), "--version".to_string()]);
    }

    #[test]
    fn unknown_positional_yields_no_candidates() {
        let tree = profile();
        let out = cands(&tree, &["pack", "bogus", ""]);
        assert!(out.is_empty());
    }

    #[test]
    fn positional_value_of_known_option_is_not_a_subcommand() {
        // --output takes a value; "json" must not be mistaken for a subcommand,
        // so we stay at the root and pack remains a candidate.
        let tree = profile();
        let out = cands(&tree, &["--output", "json", ""]);
        assert!(out.contains(&"pack".to_string()));
    }

    #[test]
    fn real_tree_covers_actual_command_surface() {
        let out = real_tree_candidates(&["pack".into(), "".into()]);
        assert!(out.contains(&"create".to_string()));
        assert!(out.contains(&"list".to_string()));
        assert!(out.contains(&"--profile".to_string()));

        let out = real_tree_candidates(&["workflow".into(), "".into()]);
        assert!(!out.is_empty());

        let out = real_tree_candidates(&["execution".into(), "".into()]);
        assert!(!out.is_empty());

        // hidden __complete must never surface
        let out = real_tree_candidates(&["".into()]);
        assert!(!out.contains(&"__complete".to_string()));
    }
}
