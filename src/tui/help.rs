//! Help catalogue: the single source of truth for every documented
//! keybinding and `:`-command rendered in the modal help overlay.
//!
//! The modal overlay (drawn by [`super::render`]) and the unit drift
//! tests in this module both read from [`HELP_ROWS`]. Add a row here
//! whenever a new key is bound or a `:`-command is added; the drift
//! test in this file guarantees the parser's `COMMAND_NAMES` list and
//! the overlay stay aligned.
//!
//! `Applicability` records the pane/mode each row applies in so the
//! overlay (and future per-pane filters) can describe context even
//! while the runtime keymap is overloaded across panes. Documenting
//! the overload is intentional — the renderer never changes runtime
//! behaviour.

/// Pane or mode that an individual [`HelpEntry`] documents.
///
/// The variants intentionally mirror [`super::app::ActivePane`] and
/// [`super::app::InputMode`] so the drift tests can group rows by
/// scope without re-encoding the dispatch logic. The renderer
/// surfaces this field alongside each row's section header so users
/// can match the chord to where it applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Applicability {
    /// Active in every normal-mode pane (Tab, q, refresh, etc.).
    Global,
    /// Conversations / thread list (regular mail folders only).
    Conversations,
    /// Conversations pane while the selected folder is a Drafts
    /// folder (role = `drafts`). Mirrors the keymap-dispatch variant
    /// `super::PaneKind::ConversationsDrafts`; update both when
    /// adding pane-scoped help rows for drafts.
    ConversationsDrafts,
    /// Message detail and preview pane.
    Details,
    /// Attachments list and preview pane.
    Attachments,
    /// Approvals virtual folder (key behaviour differs from regular
    /// mail folders).
    Approvals,
    /// Composer (Compose / ComposeAttachPath input modes).
    Composer,
    /// `:`-mode command bar.
    CommandBar,
    /// `/`-mode quick search input.
    QuickSearch,
}

/// A single documented binding inside the help overlay.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HelpEntry {
    /// Human-readable key chord ("Ctrl-P", "j / k", "Tab", "?"). May
    /// embed multiple chords separated by " / " when the same action
    /// is reachable via several keys.
    pub(crate) keys: &'static str,
    /// One-line summary describing the effect of the chord.
    pub(crate) summary: &'static str,
    /// Pane or mode the chord applies in. Drives section grouping in
    /// the overlay and the drift tests; Slice 2 will use it to filter
    /// rows when the help overlay is opened with a specific pane
    /// focused. Read by drift tests only today.
    #[allow(dead_code)]
    pub(crate) applies_to: Applicability,
}

/// A grouping of [`HelpEntry`] rendered under a single header in the
/// overlay.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HelpSection {
    /// Title of the section as rendered in the overlay.
    pub(crate) title: &'static str,
    /// Rows for this section. Order is preserved in the overlay.
    pub(crate) entries: &'static [HelpEntry],
}

/// Canonical, source-of-truth set of help sections in their render
/// order. The drift tests assert this ordering and that every
/// [`super::command::COMMAND_NAMES`] entry shows up somewhere here.
pub(crate) static HELP_ROWS: &[HelpSection] = &[
    HelpSection {
        title: "Panes & navigation",
        entries: &[
            HelpEntry {
                keys: "Tab",
                summary: "Cycle focus to the next pane",
                applies_to: Applicability::Global,
            },
            HelpEntry {
                keys: "Shift-Tab / ←",
                summary: "Cycle focus to the previous pane (Left is the same)",
                applies_to: Applicability::Global,
            },
            HelpEntry {
                keys: "→",
                summary: "Cycle focus forward (same as Tab)",
                applies_to: Applicability::Global,
            },
            HelpEntry {
                keys: "Ctrl-P",
                summary: "Open the virtual Approvals folder (alias :approvals, :goto Approvals)",
                applies_to: Applicability::Global,
            },
            HelpEntry {
                keys: "q",
                summary: "Quit postblox (top-level normal mode only)",
                applies_to: Applicability::Global,
            },
            HelpEntry {
                keys: "r",
                summary: "Refresh the focused pane (accounts/folders/conversations/details/attachments/search)",
                applies_to: Applicability::Global,
            },
            HelpEntry {
                keys: "? / :help",
                summary: "Toggle this help overlay (Esc also closes it)",
                applies_to: Applicability::Global,
            },
        ],
    },
    HelpSection {
        title: "Conversations (mail folders)",
        entries: &[
            HelpEntry {
                keys: "↑ / ↓ or j / k",
                summary: "Move selection up / down in the focused list",
                applies_to: Applicability::Conversations,
            },
            HelpEntry {
                keys: "Enter",
                summary: "Open the focused thread (refresh detail / focus preview)",
                applies_to: Applicability::Conversations,
            },
            HelpEntry {
                keys: "c / :compose",
                summary: "Compose a new message in the active account",
                applies_to: Applicability::Conversations,
            },
            HelpEntry {
                keys: "R / :reply",
                summary: "Reply to the selected message",
                applies_to: Applicability::Conversations,
            },
            HelpEntry {
                keys: "A / :reply-all",
                summary: "Reply-all to the selected message",
                applies_to: Applicability::Conversations,
            },
            HelpEntry {
                keys: "F / :forward",
                summary: "Forward the selected message (carries attachments)",
                applies_to: Applicability::Conversations,
            },
            HelpEntry {
                keys: "u / :seen / :unseen",
                summary: "Toggle the Seen flag on the selected message",
                applies_to: Applicability::Conversations,
            },
            HelpEntry {
                keys: "f / * / :flag / :unflag",
                summary: "Toggle the Flagged flag on the selected message",
                applies_to: Applicability::Conversations,
            },
            HelpEntry {
                keys: "a",
                summary: "Toggle the attachments pane for the selected message",
                applies_to: Applicability::Conversations,
            },
            HelpEntry {
                keys: "d / :delete",
                summary: "Open delete-confirm modal (y/n) for the selected message",
                applies_to: Applicability::Conversations,
            },
            HelpEntry {
                keys: "e / :archive",
                summary: "Archive the selected message",
                applies_to: Applicability::Conversations,
            },
            HelpEntry {
                keys: "m / :move <folder>",
                summary: "Open command bar pre-filled with `move `",
                applies_to: Applicability::Conversations,
            },
        ],
    },
    HelpSection {
        title: "Conversations (drafts folder)",
        entries: &[
            HelpEntry {
                keys: "Enter / o",
                summary: "Open the highlighted draft in the composer",
                applies_to: Applicability::ConversationsDrafts,
            },
            HelpEntry {
                keys: "d",
                summary: "Open delete-confirm modal (y/n) for the highlighted draft",
                applies_to: Applicability::ConversationsDrafts,
            },
        ],
    },
    HelpSection {
        title: "Details / conversation stack",
        entries: &[
            HelpEntry {
                keys: "j / k",
                summary: "Step to next / previous message in the stack, then scroll text",
                applies_to: Applicability::Details,
            },
            HelpEntry {
                keys: "o",
                summary: "Toggle expansion of the focused message",
                applies_to: Applicability::Details,
            },
            HelpEntry {
                keys: "O",
                summary: "Expand every message in the conversation stack",
                applies_to: Applicability::Details,
            },
            HelpEntry {
                keys: "a",
                summary: "Toggle the attachments pane for the focused message",
                applies_to: Applicability::Details,
            },
            HelpEntry {
                keys: "d",
                summary: "Open delete-confirm modal (y/n) for the focused message",
                applies_to: Applicability::Details,
            },
            HelpEntry {
                keys: "e",
                summary: "Archive the focused message",
                applies_to: Applicability::Details,
            },
            HelpEntry {
                keys: "m",
                summary: "Open command bar pre-filled with `move `",
                applies_to: Applicability::Details,
            },
            HelpEntry {
                keys: "PgUp / PgDn",
                summary: "Page the detail / preview / composer body viewport",
                applies_to: Applicability::Details,
            },
            HelpEntry {
                keys: "v",
                summary: "Toggle line-select in detail / preview / composer body",
                applies_to: Applicability::Details,
            },
            HelpEntry {
                keys: "y",
                summary: "Copy the active selection (preview / detail focus)",
                applies_to: Applicability::Details,
            },
            HelpEntry {
                keys: "Esc",
                summary: "Clear detail selection (when one is active)",
                applies_to: Applicability::Details,
            },
        ],
    },
    HelpSection {
        title: "Attachments",
        entries: &[
            HelpEntry {
                keys: "Enter",
                summary: "Focus the preview pane (j/k scrolls, v selects, y copies)",
                applies_to: Applicability::Attachments,
            },
            HelpEntry {
                keys: "o",
                summary: "Open the selected attachment with xdg-open (Linux, y/n confirm)",
                applies_to: Applicability::Attachments,
            },
            HelpEntry {
                keys: "a",
                summary: "Close the attachments pane (toggle back to Conversations)",
                applies_to: Applicability::Attachments,
            },
            HelpEntry {
                keys: "e",
                summary: "Export the selected attachment to the working directory",
                applies_to: Applicability::Attachments,
            },
        ],
    },
    HelpSection {
        title: "Approvals",
        entries: &[
            HelpEntry {
                keys: "Ctrl-P / :approvals",
                summary: "Open the virtual Approvals folder (alias :goto Approvals)",
                applies_to: Applicability::Approvals,
            },
            HelpEntry {
                keys: "a / :approve",
                summary: "Approve the highlighted pending approval",
                applies_to: Applicability::Approvals,
            },
            HelpEntry {
                keys: "d / :deny",
                summary: "Deny the highlighted pending approval",
                applies_to: Applicability::Approvals,
            },
        ],
    },
    HelpSection {
        title: "Search & sync",
        entries: &[
            HelpEntry {
                keys: "/",
                summary: "Quick-search the active account (Enter submits, Esc cancels)",
                applies_to: Applicability::QuickSearch,
            },
            HelpEntry {
                keys: ":search [--account <name>] <query>",
                summary: "Run an FTS5 search (optionally scoped to a single account)",
                applies_to: Applicability::CommandBar,
            },
            HelpEntry {
                keys: "s / :sync",
                summary: "Sync the active folder (one-shot reconcile)",
                applies_to: Applicability::Global,
            },
            HelpEntry {
                keys: ":start-sync / :stop-sync",
                summary: "Start or stop the IMAP IDLE worker for the active folder",
                applies_to: Applicability::Global,
            },
            HelpEntry {
                keys: "x / X",
                summary: "Dismiss newest toast / clear all toasts",
                applies_to: Applicability::Global,
            },
        ],
    },
    HelpSection {
        title: "Composer",
        entries: &[
            HelpEntry {
                keys: "Tab / Shift-Tab",
                summary: "Next / previous composer field",
                applies_to: Applicability::Composer,
            },
            HelpEntry {
                keys: "Ctrl-A",
                summary: "Prompt for a file path to attach (Esc cancels the prompt)",
                applies_to: Applicability::Composer,
            },
            HelpEntry {
                keys: "Ctrl-K",
                summary: "Remove the highlighted attachment",
                applies_to: Applicability::Composer,
            },
            HelpEntry {
                keys: "Ctrl-S / :w",
                summary: "Save draft (:w is a vim-style alias from non-body fields)",
                applies_to: Applicability::Composer,
            },
            HelpEntry {
                keys: "Ctrl-X",
                summary: "Send draft",
                applies_to: Applicability::Composer,
            },
            HelpEntry {
                keys: "Esc",
                summary: "Cancel composer (confirms y/n when the draft is dirty)",
                applies_to: Applicability::Composer,
            },
        ],
    },
    HelpSection {
        title: "Theme",
        entries: &[
            HelpEntry {
                keys: "t / Ctrl-T",
                summary: "Rotate light → dark → high-contrast",
                applies_to: Applicability::Global,
            },
            HelpEntry {
                keys: ":theme next | light | dark | high-contrast",
                summary: "Switch directly to a theme by name (`hc` is an alias for high-contrast)",
                applies_to: Applicability::Global,
            },
        ],
    },
    HelpSection {
        title: "Command bar (`:`)",
        entries: &[
            HelpEntry {
                keys: ":",
                summary: "Open the command bar (also reachable from non-body composer fields)",
                applies_to: Applicability::CommandBar,
            },
            HelpEntry {
                keys: "Tab",
                summary: "Complete the longest unambiguous command prefix",
                applies_to: Applicability::CommandBar,
            },
            HelpEntry {
                keys: "Enter",
                summary: "Run the command (empty input is a silent no-op)",
                applies_to: Applicability::CommandBar,
            },
            HelpEntry {
                keys: "Esc",
                summary: "Cancel and return to Normal (or Compose if entered from there)",
                applies_to: Applicability::CommandBar,
            },
            HelpEntry {
                keys: ":account <name|email>",
                summary: "Switch the active account by display name or email",
                applies_to: Applicability::CommandBar,
            },
            HelpEntry {
                keys: ":goto <folder>",
                summary: "Switch the active folder by name (case-insensitive fallback; `:goto Approvals` aliases :approvals)",
                applies_to: Applicability::CommandBar,
            },
            HelpEntry {
                keys: ":help",
                summary: "Open this help overlay",
                applies_to: Applicability::CommandBar,
            },
        ],
    },
];

/// Canonical, ordered section titles. Kept separate from
/// [`HELP_ROWS`] so the drift test in this module can assert order and
/// coverage without iterating the static section list at runtime.
#[cfg(test)]
pub(crate) const HELP_SECTION_TITLES: &[&str] = &[
    "Panes & navigation",
    "Conversations (mail folders)",
    "Conversations (drafts folder)",
    "Details / conversation stack",
    "Attachments",
    "Approvals",
    "Search & sync",
    "Composer",
    "Theme",
    "Command bar (`:`)",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::command::COMMAND_NAMES;

    /// Every documented `:command` reference in the help overlay must
    /// resolve to a real command in the parser's COMMAND_NAMES list.
    /// Add the command to the parser first, then add the row here.
    #[test]
    fn test_help_rows_reference_only_known_commands() {
        for section in HELP_ROWS {
            for entry in section.entries {
                for token in entry.keys.split_whitespace() {
                    let Some(stripped) = token.strip_prefix(':') else {
                        continue;
                    };
                    // Strip trailing punctuation injected for prose like
                    // ":approve," or ":approvals)". The parser names are
                    // pure ASCII identifiers, so a non-alpha stop is fine.
                    let cleaned: String = stripped
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                        .collect();
                    if cleaned.is_empty() {
                        continue;
                    }
                    if cleaned == "help" {
                        // The `help` row is in this slice but only lands
                        // in COMMAND_NAMES once Slice 1 ships the parser
                        // hook. The other drift test below covers the
                        // command_names → HELP_ROWS direction.
                        continue;
                    }
                    assert!(
                        COMMAND_NAMES.contains(&cleaned.as_str()),
                        "help references unknown command :{} in row '{}'",
                        cleaned,
                        entry.keys
                    );
                }
            }
        }
    }

    /// Drift test: every name in [`COMMAND_NAMES`] must appear in at
    /// least one help row's `keys` field (substring match against
    /// `":<name>"`). This is the primary guard against the parser and
    /// the overlay drifting apart over time.
    #[test]
    fn test_every_command_name_is_documented_in_help_rows() {
        let mut missing: Vec<&str> = Vec::new();
        for name in COMMAND_NAMES {
            let needle = format!(":{name}");
            let found = HELP_ROWS.iter().any(|section| {
                section
                    .entries
                    .iter()
                    .any(|entry| entry.keys.contains(needle.as_str()))
            });
            if !found {
                missing.push(*name);
            }
        }
        assert!(
            missing.is_empty(),
            "command names missing from HELP_ROWS: {:?}",
            missing
        );
    }

    /// Section titles must match the canonical list in
    /// [`HELP_SECTION_TITLES`], in order. This catches accidental
    /// rename / reorder commits that would silently scramble the
    /// overlay layout.
    #[test]
    fn test_help_sections_match_canonical_order() {
        let actual: Vec<&str> = HELP_ROWS.iter().map(|s| s.title).collect();
        assert_eq!(actual.as_slice(), HELP_SECTION_TITLES);
    }

    /// Every section header must have at least one row so the renderer
    /// never produces an empty `Section\n` block.
    #[test]
    fn test_every_help_section_has_at_least_one_row() {
        for section in HELP_ROWS {
            assert!(
                !section.entries.is_empty(),
                "section '{}' has no entries",
                section.title
            );
        }
    }

    /// Cheap canary: the overlay must include the long-form information
    /// that used to live in the bottom status bar manual. If a future
    /// refactor scrubs the canary string, the bottom-bar contraction
    /// test in `render.rs` will pin the new wording.
    #[test]
    fn test_help_rows_cover_export_and_archive_overload() {
        let combined: String = HELP_ROWS
            .iter()
            .flat_map(|s| s.entries.iter())
            .map(|e| e.summary.to_lowercase())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            combined.contains("export"),
            "help overlay must document the export side of `e`"
        );
        assert!(
            combined.contains("archive"),
            "help overlay must document the archive side of `e`"
        );
    }

    /// Smoke test that the `Applicability` taxonomy is exercised: every
    /// pane variant must be referenced by at least one row.
    /// Catches accidental enum-variant rename / removal that would
    /// silently strand whole panes from the help catalogue.
    #[test]
    fn test_help_rows_exercise_every_applicability_variant() {
        let mut seen = std::collections::HashSet::new();
        for section in HELP_ROWS {
            for entry in section.entries {
                seen.insert(entry.applies_to);
            }
        }
        for variant in [
            Applicability::Global,
            Applicability::Conversations,
            Applicability::ConversationsDrafts,
            Applicability::Details,
            Applicability::Attachments,
            Applicability::Approvals,
            Applicability::Composer,
            Applicability::CommandBar,
            Applicability::QuickSearch,
        ] {
            assert!(
                seen.contains(&variant),
                "no help row applies to {variant:?}"
            );
        }
    }

    /// Every overloaded single-letter chord (`o`/`a`/`d`/`e`/`m`) must
    /// be documented under a concrete pane, never as `Global`. The
    /// dispatcher in `src/tui/mod.rs::resolve_pane_action` resolves
    /// each chord to one action per pane; the overlay must mirror the
    /// table so the rows the user sees reflect where the binding is
    /// active.
    #[test]
    fn test_pane_overloaded_chords_are_not_global() {
        let chords = ["o", "a", "d", "e", "m"];
        for section in HELP_ROWS {
            for entry in section.entries {
                for token in entry.keys.split('/').map(str::trim) {
                    let head = token.split_whitespace().next().unwrap_or("");
                    if chords.contains(&head) {
                        assert_ne!(
                            entry.applies_to,
                            Applicability::Global,
                            "row '{}' uses overloaded chord '{}' but is Global; \
                             pin the row to its pane",
                            entry.keys,
                            head,
                        );
                    }
                }
            }
        }
    }

    /// Approvals section must surface the approve/deny verbs so users
    /// can self-rescue after the overload reversal (`a` = approve here,
    /// not toggle attachments; `d` = deny, not delete).
    #[test]
    fn test_approvals_section_has_approve_and_deny_rows() {
        let approvals: Vec<&HelpEntry> = HELP_ROWS
            .iter()
            .filter(|s| s.title == "Approvals")
            .flat_map(|s| s.entries.iter())
            .collect();
        assert!(
            approvals
                .iter()
                .any(|e| e.keys.contains('a') && e.summary.to_lowercase().contains("approve")),
            "Approvals section missing an `a approve` row: {approvals:?}"
        );
        assert!(
            approvals
                .iter()
                .any(|e| e.keys.contains('d') && e.summary.to_lowercase().contains("deny")),
            "Approvals section missing a `d deny` row: {approvals:?}"
        );
    }

    /// Drafts use of the Conversations pane has its own keymap (`o` /
    /// `Enter` reopen; `d` deletes the highlighted draft). Document
    /// both so the overlay matches the runtime dispatch.
    #[test]
    fn test_drafts_section_has_open_and_delete_rows() {
        let drafts: Vec<&HelpEntry> = HELP_ROWS
            .iter()
            .filter(|s| s.title == "Conversations (drafts folder)")
            .flat_map(|s| s.entries.iter())
            .collect();
        assert!(
            drafts
                .iter()
                .any(|e| e.keys.contains('o') && e.summary.to_lowercase().contains("draft")),
            "Drafts section missing an `o open draft` row: {drafts:?}"
        );
        assert!(
            drafts
                .iter()
                .any(|e| e.keys.contains('d') && e.summary.to_lowercase().contains("draft")),
            "Drafts section missing a `d delete draft` row: {drafts:?}"
        );
    }

    /// Attachments section must surface `o` (xdg-open) and `e` (export)
    /// so users don't have to discover the export verb by mashing keys.
    #[test]
    fn test_attachments_section_has_open_and_export_rows() {
        let attachments: Vec<&HelpEntry> = HELP_ROWS
            .iter()
            .filter(|s| s.title == "Attachments")
            .flat_map(|s| s.entries.iter())
            .collect();
        assert!(
            attachments
                .iter()
                .any(|e| e.keys.contains('o') && e.summary.to_lowercase().contains("open")),
            "Attachments section missing an `o open file` row: {attachments:?}"
        );
        assert!(
            attachments
                .iter()
                .any(|e| e.keys.contains('e') && e.summary.to_lowercase().contains("export")),
            "Attachments section missing an `e export` row: {attachments:?}"
        );
    }
}
