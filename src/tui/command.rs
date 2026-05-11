//! Parser and tab-completion for the TUI's `:`-mode command bar.
//!
//! [`parse_command`] turns a `:`-line into a [`Command`] enum that
//! [`super::app::AppState`] dispatches. Vim-style aliases (`:w` for
//! `Write`) keep muscle memory intact. `COMMAND_NAMES` is the
//! sorted source of truth for tab completion — keeping it sorted
//! gives deterministic match ordering. Errors are flat, lowercase
//! [`CommandError`] variants per AGENTS.md.

use thiserror::Error;

use super::theme::ThemeName;

/// Parsed `:`-mode command dispatched by the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `:sync` — run a one-shot reconcile of the active folder.
    Sync,
    /// `:start-sync` — start the IMAP IDLE worker for the active folder.
    StartSync,
    /// `:stop-sync` — stop the IMAP IDLE worker for the active folder.
    StopSync,
    /// `:seen` — mark the selected message as read.
    Seen,
    /// `:unseen` — mark the selected message as unread.
    Unseen,
    /// `:flag` — flag the selected message.
    Flag,
    /// `:unflag` — clear the flag on the selected message.
    Unflag,
    /// `:archive` — move the selected message to the archive folder.
    Archive,
    /// `:approvals` — select the virtual pending MCP approvals folder.
    Approvals,
    /// `:approve` — allow the highlighted pending approval.
    Approve,
    /// `:delete` — move the selected message to the trash folder.
    Delete,
    /// `:deny` — deny the highlighted pending approval.
    Deny,
    /// `:move <folder>` — move the selected message to `<folder>`.
    Move(String),
    /// `:theme next` — advance to the next theme in the rotation.
    ThemeNext,
    /// `:theme <name>` — switch directly to the named theme.
    Theme(ThemeName),
    /// `:compose` — open a blank composer for a new message.
    Compose,
    /// `:reply` — open the composer pre-filled with a reply.
    Reply,
    /// `:reply-all` — open the composer pre-filled with a reply-all.
    ReplyAll,
    /// `:forward` — open the composer pre-filled with a forward.
    Forward,
    /// `:goto <folder>` — switch the active folder to `<folder>`.
    Goto(String),
    /// `:help` — open the modal help overlay (mirrors the `?` chord).
    Help,
    /// `:account <name|email>` — switch the active account.
    Account(String),
    /// `:search [--account <name>] <query>` — run an FTS5 search.
    Search {
        /// Optional account-name filter (matches display name or email).
        account: Option<String>,
        /// Free-text query passed to the daemon's search op.
        query: String,
    },
    /// Persist the current composer draft (alias `:w`). Mirrors the
    /// `Ctrl-S` keybinding so users with vim muscle-memory don't have
    /// to learn the chord.
    Write,
}

/// Names recognized as commands at the start of a `:`-line. Sorted so
/// Tab-completion has a deterministic match order.
pub(crate) const COMMAND_NAMES: &[&str] = &[
    "account",
    "approvals",
    "approve",
    "archive",
    "compose",
    "delete",
    "deny",
    "flag",
    "forward",
    "goto",
    "help",
    "move",
    "reply",
    "reply-all",
    "search",
    "seen",
    "start-sync",
    "stop-sync",
    "sync",
    "theme",
    "unflag",
    "unseen",
    "w",
];

/// Errors returned by [`parse_command`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommandError {
    /// Input had no command name after trimming whitespace.
    #[error("empty command")]
    Empty,
    /// Leading token is not a recognised command name.
    #[error("unknown command '{0}'")]
    Unknown(String),
    /// Command name parsed but arguments did not match the expected shape.
    #[error("usage: {0}")]
    Usage(&'static str),
}

/// Parse a `:`-mode command line into a [`Command`].
///
/// # Errors
///
/// Returns:
/// - [`CommandError::Empty`] if `input` is empty after trimming.
/// - [`CommandError::Unknown`] if the leading token is not a known
///   command name.
/// - [`CommandError::Usage`] if the arguments don't match the
///   command's expected shape (missing folder for `move`, extra args
///   for argless commands, invalid theme name, etc.).
pub fn parse_command(input: &str) -> Result<Command, CommandError> {
    let mut parts = input.split_whitespace();
    let Some(name) = parts.next() else {
        return Err(CommandError::Empty);
    };

    match name {
        "sync" => parse_no_args(Command::Sync, "sync", parts),
        "start-sync" => parse_no_args(Command::StartSync, "start-sync", parts),
        "stop-sync" => parse_no_args(Command::StopSync, "stop-sync", parts),
        "seen" => parse_no_args(Command::Seen, "seen", parts),
        "unseen" => parse_no_args(Command::Unseen, "unseen", parts),
        "flag" => parse_no_args(Command::Flag, "flag", parts),
        "unflag" => parse_no_args(Command::Unflag, "unflag", parts),
        "archive" => parse_no_args(Command::Archive, "archive", parts),
        "approvals" => parse_no_args(Command::Approvals, "approvals", parts),
        "approve" => parse_no_args(Command::Approve, "approve", parts),
        "delete" => parse_no_args(Command::Delete, "delete", parts),
        "deny" => parse_no_args(Command::Deny, "deny", parts),
        "compose" => parse_no_args(Command::Compose, "compose", parts),
        "reply" => parse_no_args(Command::Reply, "reply", parts),
        "reply-all" => parse_no_args(Command::ReplyAll, "reply-all", parts),
        "forward" => parse_no_args(Command::Forward, "forward", parts),
        "help" => parse_no_args(Command::Help, "help", parts),
        "w" => parse_no_args(Command::Write, "w", parts),
        "move" => parse_remainder(input, "move", parts).map(Command::Move),
        "goto" => parse_remainder(input, "goto", parts).map(Command::Goto),
        "account" => parse_account(parts),
        "search" => parse_search(parts),
        "theme" => parse_theme(parts),
        other => Err(CommandError::Unknown(other.to_string())),
    }
}

/// Return the closest known command name to `input` within
/// Damerau-Levenshtein distance ≤ 2, used by the TUI to attach a
/// `did you mean :foo?` suggestion to [`CommandError::Unknown`].
///
/// Ties are broken by ascending edit distance first, then by the
/// order of [`COMMAND_NAMES`] (which is sorted ASCII-ascending — so
/// the lexicographically earlier name wins on a tie).
///
/// Returns `None` when:
/// - `input` is empty (parser already rejects this; defensive),
/// - no command name is within distance 2, or
/// - the length difference between `input` and every candidate is
///   greater than 2 (a fast short-circuit before computing the
///   matrix).
///
/// Distance 0 means `input` exactly matches a command name — the
/// `parse_command` parser handles that branch on its own, so callers
/// reaching this helper from `record_command_parse_error` will never
/// hit distance 0 in practice. The helper still returns the exact
/// match in that case so it is correct in isolation.
pub(crate) fn nearest_command_name(input: &str) -> Option<&'static str> {
    if input.is_empty() {
        return None;
    }
    let input_bytes = input.as_bytes();
    let input_len = input_bytes.len();
    let mut best: Option<(usize, usize)> = None;
    for (index, name) in COMMAND_NAMES.iter().enumerate() {
        let name_bytes = name.as_bytes();
        let len_diff = name_bytes.len().abs_diff(input_len);
        if len_diff > 2 {
            continue;
        }
        let distance = damerau_levenshtein(input_bytes, name_bytes);
        if distance > 2 {
            continue;
        }
        match best {
            None => best = Some((distance, index)),
            Some((current_distance, current_index)) => {
                if distance < current_distance
                    || (distance == current_distance && index < current_index)
                {
                    best = Some((distance, index));
                }
            }
        }
    }
    best.map(|(_, index)| COMMAND_NAMES[index])
}

/// Damerau-Levenshtein distance over ASCII byte slices. The command
/// names are ASCII, so byte comparison is sufficient. Two rolling
/// rows + a previous-previous row handle the transposition case.
fn damerau_levenshtein(a: &[u8], b: &[u8]) -> usize {
    let (rows, cols) = (a.len(), b.len());
    if rows == 0 {
        return cols;
    }
    if cols == 0 {
        return rows;
    }
    let mut prev_prev: Vec<usize> = vec![0; cols + 1];
    let mut prev: Vec<usize> = (0..=cols).collect();
    let mut curr: Vec<usize> = vec![0; cols + 1];
    for i in 1..=rows {
        curr[0] = i;
        for j in 1..=cols {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let deletion = prev[j] + 1;
            let insertion = curr[j - 1] + 1;
            let substitution = prev[j - 1] + cost;
            let mut value = deletion.min(insertion).min(substitution);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                value = value.min(prev_prev[j - 2] + 1);
            }
            curr[j] = value;
        }
        std::mem::swap(&mut prev_prev, &mut prev);
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[cols]
}

/// Return the longest unambiguous Tab-completion for a `:`-mode prefix.
///
/// - `Some((completion, single_match))` when the prefix has at least one
///   match. `completion` is the longest common prefix of all matches; if
///   there is a single match, `single_match` is true so callers can
///   append a trailing space. The `prefix` is matched against the
///   command name only — args are ignored (they aren't completed yet).
/// - `None` when the input already contains whitespace (i.e. the user
///   has moved past the command name into args) or when there are no
///   matches.
pub(crate) fn complete_command(input: &str) -> Option<CommandCompletion> {
    if input.is_empty() {
        return None;
    }
    if input.contains(char::is_whitespace) {
        return None;
    }
    let matches: Vec<&str> = COMMAND_NAMES
        .iter()
        .copied()
        .filter(|name| name.starts_with(input))
        .collect();
    match matches.as_slice() {
        [] => None,
        [only] => Some(CommandCompletion {
            text: (*only).to_string(),
            matches: vec![(*only).to_string()],
            unique: true,
        }),
        many => {
            let common = longest_common_prefix(many);
            Some(CommandCompletion {
                text: common,
                matches: many.iter().map(|s| (*s).to_string()).collect(),
                unique: false,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandCompletion {
    /// Replacement text for the input prefix (longest common match).
    pub(crate) text: String,
    /// All matching command names in lexical order.
    pub(crate) matches: Vec<String>,
    /// True when there is exactly one matching command name.
    pub(crate) unique: bool,
}

fn longest_common_prefix(words: &[&str]) -> String {
    let mut prefix = String::new();
    let Some(first) = words.first() else {
        return prefix;
    };
    for (i, ch) in first.chars().enumerate() {
        if words
            .iter()
            .all(|word| word.chars().nth(i).is_some_and(|c| c == ch))
        {
            prefix.push(ch);
        } else {
            break;
        }
    }
    prefix
}

fn parse_no_args<'a>(
    command: Command,
    usage: &'static str,
    mut parts: impl Iterator<Item = &'a str>,
) -> Result<Command, CommandError> {
    if parts.next().is_some() {
        Err(CommandError::Usage(usage))
    } else {
        Ok(command)
    }
}

/// Parse a command name followed by everything else as a single
/// trimmed string, used for `:move <folder>` and `:goto <folder>` so
/// folder names with spaces survive.
fn parse_remainder<'a>(
    input: &str,
    token: &'static str,
    parts: impl Iterator<Item = &'a str>,
) -> Result<String, CommandError> {
    let collected: Vec<&str> = parts.collect();
    if collected.is_empty() {
        return Err(usage_for(token));
    }
    let value = remainder_after_token(input, token).trim().to_string();
    if value.is_empty() {
        return Err(usage_for(token));
    }
    Ok(value)
}

fn usage_for(token: &str) -> CommandError {
    match token {
        "move" => CommandError::Usage("move <folder>"),
        "goto" => CommandError::Usage("goto <folder>"),
        _ => CommandError::Usage("missing argument"),
    }
}

fn parse_account<'a>(mut parts: impl Iterator<Item = &'a str>) -> Result<Command, CommandError> {
    let Some(name) = parts.next() else {
        return Err(CommandError::Usage("account <name|email>"));
    };
    if parts.next().is_some() {
        return Err(CommandError::Usage("account <name|email>"));
    }
    Ok(Command::Account(name.to_string()))
}

fn parse_search<'a>(mut parts: impl Iterator<Item = &'a str>) -> Result<Command, CommandError> {
    let mut account: Option<String> = None;
    let mut query_parts: Vec<&str> = Vec::new();
    while let Some(part) = parts.next() {
        if part == "--account" {
            let Some(value) = parts.next() else {
                return Err(CommandError::Usage("search [--account <name>] <query>"));
            };
            if account.is_some() {
                return Err(CommandError::Usage("search [--account <name>] <query>"));
            }
            account = Some(value.to_string());
        } else {
            query_parts.push(part);
        }
    }
    if query_parts.is_empty() {
        return Err(CommandError::Usage("search [--account <name>] <query>"));
    }
    let query = query_parts.join(" ");
    Ok(Command::Search { account, query })
}

/// Return everything after the first occurrence of `token`. Used for
/// `:move <folder>` so that folder names with spaces survive parsing.
fn remainder_after_token(input: &str, token: &str) -> String {
    let trimmed = input.trim_start();
    if let Some(rest) = trimmed.strip_prefix(token) {
        rest.to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_theme<'a>(mut parts: impl Iterator<Item = &'a str>) -> Result<Command, CommandError> {
    let Some(name) = parts.next() else {
        return Err(CommandError::Usage("theme next|light|dark|high-contrast"));
    };
    if parts.next().is_some() {
        return Err(CommandError::Usage("theme next|light|dark|high-contrast"));
    }

    if name == "next" {
        Ok(Command::ThemeNext)
    } else {
        name.parse::<ThemeName>()
            .map(Command::Theme)
            .map_err(|_| CommandError::Usage("theme next|light|dark|high-contrast"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command_accepts_write_actions_without_arguments() {
        assert_eq!(parse_command("sync").unwrap(), Command::Sync);
        assert_eq!(parse_command("start-sync").unwrap(), Command::StartSync);
        assert_eq!(parse_command("stop-sync").unwrap(), Command::StopSync);
        assert_eq!(parse_command("seen").unwrap(), Command::Seen);
        assert_eq!(parse_command("unseen").unwrap(), Command::Unseen);
        assert_eq!(parse_command("flag").unwrap(), Command::Flag);
        assert_eq!(parse_command("unflag").unwrap(), Command::Unflag);
    }

    #[test]
    fn test_parse_command_trims_surrounding_whitespace() {
        assert_eq!(parse_command("  sync  ").unwrap(), Command::Sync);
    }

    #[test]
    fn test_parse_command_accepts_theme_commands() {
        assert_eq!(parse_command("theme next").unwrap(), Command::ThemeNext);
        assert_eq!(
            parse_command("theme light").unwrap(),
            Command::Theme(ThemeName::Light)
        );
        assert_eq!(
            parse_command("theme dark").unwrap(),
            Command::Theme(ThemeName::Dark)
        );
        assert_eq!(
            parse_command("theme high-contrast").unwrap(),
            Command::Theme(ThemeName::HighContrast)
        );
        assert_eq!(
            parse_command("theme hc").unwrap(),
            Command::Theme(ThemeName::HighContrast)
        );
    }

    #[test]
    fn test_parse_command_rejects_empty_input() {
        let err = parse_command("   ").unwrap_err();

        assert_eq!(err, CommandError::Empty);
        assert_eq!(err.to_string(), "empty command");
    }

    #[test]
    fn test_parse_command_rejects_unknown_command() {
        let err = parse_command("delete-everything").unwrap_err();

        assert_eq!(err, CommandError::Unknown("delete-everything".to_string()));
    }

    #[test]
    fn test_parse_command_rejects_extra_arguments() {
        let err = parse_command("sync now").unwrap_err();

        assert_eq!(err, CommandError::Usage("sync"));
        assert_eq!(err.to_string(), "usage: sync");
    }

    #[test]
    fn test_parse_command_rejects_invalid_theme_usage() {
        let err = parse_command("theme solarized").unwrap_err();

        assert_eq!(
            err,
            CommandError::Usage("theme next|light|dark|high-contrast")
        );
    }

    #[test]
    fn test_parse_command_archive_and_delete_take_no_args() {
        assert_eq!(parse_command("archive").unwrap(), Command::Archive);
        assert_eq!(parse_command("delete").unwrap(), Command::Delete);
        assert_eq!(
            parse_command("archive now").unwrap_err(),
            CommandError::Usage("archive")
        );
        assert_eq!(
            parse_command("delete now").unwrap_err(),
            CommandError::Usage("delete")
        );
    }

    #[test]
    fn test_parse_command_approvals_approve_and_deny_take_no_args() {
        assert_eq!(parse_command("approvals").unwrap(), Command::Approvals);
        assert_eq!(parse_command("approve").unwrap(), Command::Approve);
        assert_eq!(parse_command("deny").unwrap(), Command::Deny);
        assert_eq!(
            parse_command("approvals now").unwrap_err(),
            CommandError::Usage("approvals")
        );
        assert_eq!(
            parse_command("approve now").unwrap_err(),
            CommandError::Usage("approve")
        );
        assert_eq!(
            parse_command("deny now").unwrap_err(),
            CommandError::Usage("deny")
        );
    }

    #[test]
    fn test_parse_command_move_keeps_full_folder_name_including_spaces() {
        assert_eq!(
            parse_command("move Archive").unwrap(),
            Command::Move("Archive".to_string())
        );
        assert_eq!(
            parse_command("  move  My/Custom Folder  ").unwrap(),
            Command::Move("My/Custom Folder".to_string())
        );
    }

    #[test]
    fn test_parse_command_move_requires_folder() {
        assert_eq!(
            parse_command("move").unwrap_err(),
            CommandError::Usage("move <folder>")
        );
        assert_eq!(
            parse_command("move    ").unwrap_err(),
            CommandError::Usage("move <folder>")
        );
    }

    #[test]
    fn test_parse_command_compose_reply_replyall_forward_take_no_args() {
        assert_eq!(parse_command("compose").unwrap(), Command::Compose);
        assert_eq!(parse_command("reply").unwrap(), Command::Reply);
        assert_eq!(parse_command("reply-all").unwrap(), Command::ReplyAll);
        assert_eq!(parse_command("forward").unwrap(), Command::Forward);
        assert_eq!(
            parse_command("compose now").unwrap_err(),
            CommandError::Usage("compose")
        );
        assert_eq!(
            parse_command("reply target").unwrap_err(),
            CommandError::Usage("reply")
        );
    }

    #[test]
    fn test_parse_command_goto_keeps_full_folder_name_including_spaces() {
        assert_eq!(
            parse_command("goto INBOX/Receipts 2025").unwrap(),
            Command::Goto("INBOX/Receipts 2025".into())
        );
        assert_eq!(
            parse_command("  goto  Sent  Items  ").unwrap(),
            Command::Goto("Sent  Items".into())
        );
    }

    #[test]
    fn test_parse_command_goto_requires_folder() {
        assert_eq!(
            parse_command("goto").unwrap_err(),
            CommandError::Usage("goto <folder>")
        );
        assert_eq!(
            parse_command("goto    ").unwrap_err(),
            CommandError::Usage("goto <folder>")
        );
    }

    #[test]
    fn test_parse_command_move_keeps_multi_word_folder() {
        assert_eq!(
            parse_command("move INBOX/Receipts 2025").unwrap(),
            Command::Move("INBOX/Receipts 2025".into())
        );
    }

    #[test]
    fn test_parse_command_account_takes_single_token() {
        assert_eq!(
            parse_command("account Work").unwrap(),
            Command::Account("Work".into())
        );
        assert_eq!(
            parse_command("account work@example.com").unwrap(),
            Command::Account("work@example.com".into())
        );
    }

    #[test]
    fn test_parse_command_account_quoted_form_is_unsupported_today() {
        // The current parser splits on whitespace and doesn't handle
        // double-quoted strings. Document that limitation explicitly so
        // any future quoting work knows where to start. `account "Work
        // Personal"` parses as a multi-token usage error.
        let err = parse_command("account \"Work Personal\"").unwrap_err();
        assert_eq!(err, CommandError::Usage("account <name|email>"));
    }

    #[test]
    fn test_parse_command_account_requires_name() {
        assert_eq!(
            parse_command("account").unwrap_err(),
            CommandError::Usage("account <name|email>")
        );
    }

    #[test]
    fn test_parse_command_search_with_account_flag() {
        assert_eq!(
            parse_command("search --account Work foo bar").unwrap(),
            Command::Search {
                account: Some("Work".into()),
                query: "foo bar".into(),
            }
        );
    }

    #[test]
    fn test_parse_command_search_without_flag() {
        assert_eq!(
            parse_command("search foo bar baz").unwrap(),
            Command::Search {
                account: None,
                query: "foo bar baz".into(),
            }
        );
    }

    #[test]
    fn test_parse_command_search_requires_query() {
        assert_eq!(
            parse_command("search").unwrap_err(),
            CommandError::Usage("search [--account <name>] <query>")
        );
        assert_eq!(
            parse_command("search --account Work").unwrap_err(),
            CommandError::Usage("search [--account <name>] <query>")
        );
        assert_eq!(
            parse_command("search --account").unwrap_err(),
            CommandError::Usage("search [--account <name>] <query>")
        );
    }

    #[test]
    fn test_parse_command_search_rejects_duplicate_account_flag() {
        assert_eq!(
            parse_command("search --account A --account B foo").unwrap_err(),
            CommandError::Usage("search [--account <name>] <query>")
        );
    }

    #[test]
    fn test_complete_command_unique_match_returns_full_name() {
        let completion = complete_command("comp").unwrap();
        assert_eq!(completion.text, "compose");
        assert!(completion.unique);
        assert_eq!(completion.matches, vec!["compose".to_string()]);
    }

    #[test]
    fn test_complete_command_returns_longest_common_prefix_for_multiple_matches() {
        let completion = complete_command("s").unwrap();
        assert!(!completion.unique);
        assert!(completion.matches.iter().any(|m| m == "sync"));
        assert!(completion.matches.iter().any(|m| m == "search"));
        // 'search', 'seen', 'start-sync', 'stop-sync', 'sync' share 's'
        // as the only common character.
        assert_eq!(completion.text, "s");
    }

    #[test]
    fn test_complete_command_resolves_m_prefix_to_move() {
        let completion = complete_command("m").unwrap();
        assert_eq!(completion.text, "move");
        assert!(completion.unique);
    }

    #[test]
    fn test_complete_command_resolves_g_prefix_to_goto() {
        let completion = complete_command("g").unwrap();
        assert_eq!(completion.text, "goto");
        assert!(completion.unique);
    }

    #[test]
    fn test_complete_command_returns_none_for_empty_or_post_arg_input() {
        assert!(complete_command("").is_none());
        assert!(complete_command("move ").is_none());
        assert!(complete_command("search foo").is_none());
    }

    #[test]
    fn test_complete_command_returns_none_for_no_match() {
        assert!(complete_command("zzz").is_none());
    }

    #[test]
    fn test_parse_command_w_alias_for_save() {
        assert_eq!(parse_command("w").unwrap(), Command::Write);
        assert_eq!(
            parse_command("w now").unwrap_err(),
            CommandError::Usage("w")
        );
    }

    #[test]
    fn test_complete_command_w_resolves_uniquely() {
        let completion = complete_command("w").unwrap();
        assert_eq!(completion.text, "w");
        assert!(completion.unique);
    }

    #[test]
    fn test_parse_command_help_takes_no_args() {
        assert_eq!(parse_command("help").unwrap(), Command::Help);
        assert_eq!(
            parse_command("help extra").unwrap_err(),
            CommandError::Usage("help")
        );
    }

    #[test]
    fn test_complete_command_help_prefix_resolves_uniquely() {
        let completion = complete_command("hel").unwrap();
        assert_eq!(completion.text, "help");
        assert!(completion.unique);
    }

    #[test]
    fn test_command_names_remain_sorted() {
        // The tab-completion contract depends on COMMAND_NAMES being
        // sorted ASCII-ascending. Guard against a careless insert.
        let mut sorted = COMMAND_NAMES.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted.as_slice(), COMMAND_NAMES);
    }

    #[test]
    fn test_nearest_command_name_returns_close_match() {
        assert_eq!(nearest_command_name("helo"), Some("help"));
        assert_eq!(nearest_command_name("archieve"), Some("archive"));
        // Transposition: Damerau distance from "seacrh" to "search" is 1.
        assert_eq!(nearest_command_name("seacrh"), Some("search"));
    }

    #[test]
    fn test_nearest_command_name_returns_none_for_far_match() {
        assert_eq!(nearest_command_name("zzz"), None);
        // Length delta vs every command name is > 2 so the short
        // circuit fires before any matrix work.
        assert_eq!(nearest_command_name("delete-everything"), None);
    }

    #[test]
    fn test_nearest_command_name_empty_input_is_none() {
        assert_eq!(nearest_command_name(""), None);
    }

    #[test]
    fn test_nearest_command_name_breaks_ties_lex_ascending() {
        // "sn" is distance 2 from both "seen" and "sync"; "seen"
        // precedes "sync" in COMMAND_NAMES (which is sorted), so the
        // helper must surface "seen".
        assert_eq!(nearest_command_name("sn"), Some("seen"));
    }
}
