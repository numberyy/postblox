use thiserror::Error;

use super::theme::ThemeName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Sync,
    StartSync,
    StopSync,
    Seen,
    Unseen,
    Flag,
    Unflag,
    Archive,
    Delete,
    Move(String),
    ThemeNext,
    Theme(ThemeName),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommandError {
    #[error("empty command")]
    Empty,
    #[error("unknown command '{0}'")]
    Unknown(String),
    #[error("usage: {0}")]
    Usage(&'static str),
}

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
        "delete" => parse_no_args(Command::Delete, "delete", parts),
        "move" => parse_move(input, parts),
        "theme" => parse_theme(parts),
        other => Err(CommandError::Unknown(other.to_string())),
    }
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

fn parse_move<'a>(
    input: &str,
    parts: impl Iterator<Item = &'a str>,
) -> Result<Command, CommandError> {
    let collected: Vec<&str> = parts.collect();
    if collected.is_empty() {
        return Err(CommandError::Usage("move <folder>"));
    }
    let folder = remainder_after_token(input, "move").trim().to_string();
    if folder.is_empty() {
        return Err(CommandError::Usage("move <folder>"));
    }
    Ok(Command::Move(folder))
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
        return Err(CommandError::Usage("theme next|default|dark|high-contrast"));
    };
    if parts.next().is_some() {
        return Err(CommandError::Usage("theme next|default|dark|high-contrast"));
    }

    if name == "next" {
        Ok(Command::ThemeNext)
    } else {
        name.parse::<ThemeName>()
            .map(Command::Theme)
            .map_err(|_| CommandError::Usage("theme next|default|dark|high-contrast"))
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
            parse_command("theme default").unwrap(),
            Command::Theme(ThemeName::Default)
        );
        assert_eq!(
            parse_command("theme dark").unwrap(),
            Command::Theme(ThemeName::Dark)
        );
        assert_eq!(
            parse_command("theme high-contrast").unwrap(),
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
            CommandError::Usage("theme next|default|dark|high-contrast")
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
}
