//! Strip quoted history and signature blocks from a plain-text reply.
//!
//! Single entry point: [`extract_reply`]. The reply body is split on
//! `\n` and walked top-down; the first cutoff line wins:
//!
//! - `--` or `\u{2014}` (em dash) — signature separator.
//! - `On … wrote:` (single line) and the multi-line Gmail-mobile wrap.
//! - Outlook-style `From:` / `Sent:` / `To:` / `Subject:` header block.
//!
//! Lines before the cutoff that look like quotes (`>` prefix, etc.) are
//! filtered out, then the result is trimmed. Pure function, framework-
//! free, used by both the TUI and the MCP bridge to summarise a reply
//! without the original conversation.

/// Return only the newly written portion of a reply body.
///
/// # Examples
///
/// ```
/// let reply = "Sounds good.\n\nOn Mon, Alice wrote:\n> Original";
/// assert_eq!(postblox_mail::reply_extract::extract_reply(reply), "Sounds good.");
/// ```
pub fn extract_reply(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();

    let cutoff = find_cutoff(&lines);

    let kept: Vec<&str> = lines[..cutoff]
        .iter()
        .copied()
        .filter(|line| !is_quote_line(line))
        .collect();

    kept.join("\n").trim().to_string()
}

fn find_cutoff(lines: &[&str]) -> usize {
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if is_signature_separator(trimmed) {
            return i;
        }

        if is_wrote_marker(trimmed) || is_multiline_wrote_marker(lines, i) {
            return i;
        }

        if is_outlook_header_block(lines, i) {
            return i;
        }
    }
    lines.len()
}

fn is_signature_separator(trimmed: &str) -> bool {
    trimmed == "--" || trimmed == "\u{2014}" // em dash
}

fn is_wrote_marker(trimmed: &str) -> bool {
    trimmed.starts_with("On ") && trimmed.ends_with("wrote:")
}

// Gmail mobile and some clients wrap "On ... wrote:" across multiple lines.
fn is_multiline_wrote_marker(lines: &[&str], i: usize) -> bool {
    let trimmed = lines[i].trim();
    if !trimmed.starts_with("On ") {
        return false;
    }
    let end = (i + 4).min(lines.len());
    let block = &lines[i..end];
    let ends_with_wrote = block.iter().any(|line| line.trim().ends_with("wrote:"));
    // Require an address-like token somewhere in the block so a normal
    // sentence that merely starts with "On " and happens to contain a line
    // ending in "wrote:" isn't mistaken for an attribution header and the
    // user's real content trimmed away.
    let has_address = block.iter().any(|line| line.contains('@'));
    ends_with_wrote && has_address
}

fn is_outlook_header_block(lines: &[&str], i: usize) -> bool {
    if i + 3 >= lines.len() {
        return false;
    }
    let l0 = lines[i].trim();
    let l1 = lines[i + 1].trim();
    let l2 = lines[i + 2].trim();
    let l3 = lines[i + 3].trim();
    l0.starts_with("From:")
        && l1.starts_with("Sent:")
        && l2.starts_with("To:")
        && l3.starts_with("Subject:")
}

fn is_quote_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('>')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_reply_quoted_text_stripped() {
        let text = "New content here.\n\n> Old quoted text.\n> More old text.";
        let result = extract_reply(text);
        assert_eq!(result, "New content here.");
    }

    #[test]
    fn test_extract_reply_signature_stripped() {
        let text = "My message.\n\n-- \nJohn Doe\nCEO, Acme Corp";
        let result = extract_reply(text);
        assert_eq!(result, "My message.");
    }

    #[test]
    fn test_extract_reply_on_wrote_pattern() {
        let text = "Sounds good!\n\nOn Mon, 1 Jan 2026 at 10:00 AM, Foo Bar <foo@bar.com> wrote:\n> original message";
        let result = extract_reply(text);
        assert_eq!(result, "Sounds good!");
    }

    #[test]
    fn test_extract_reply_keeps_prose_starting_with_on_without_address() {
        // Starts with "On " and a later line ends in "wrote:" but there is
        // no address token, so it is real content, not an attribution line.
        let text = "On vacation I read your note.\nNothing else to add, but I wrote:\nthe rest of my actual reply.";
        let result = extract_reply(text);
        assert_eq!(result, text);
    }

    #[test]
    fn test_extract_reply_outlook_pattern() {
        let text = "Approved.\n\nFrom: colleague@example.com\nSent: Thursday, January 4, 2026\nTo: me@example.com\nSubject: Budget\n\nOriginal message here.";
        let result = extract_reply(text);
        assert_eq!(result, "Approved.");
    }

    #[test]
    fn test_extract_reply_nested_quotes() {
        let text = "My reply.\n\n> Level 1 quote\n>> Level 2 quote\n>>> Level 3 quote";
        let result = extract_reply(text);
        assert_eq!(result, "My reply.");
    }

    #[test]
    fn test_extract_reply_no_quotes_no_sig_unchanged() {
        let text = "Just a regular email with no quotes or signatures.";
        let result = extract_reply(text);
        assert_eq!(result, "Just a regular email with no quotes or signatures.");
    }

    #[test]
    fn test_extract_reply_only_quotes_returns_empty() {
        let text = "> All quoted\n> Nothing new\n>> Deeper quote";
        let result = extract_reply(text);
        assert_eq!(result, "");
    }

    #[test]
    fn test_extract_reply_mixed_content_keeps_original() {
        let text = "First paragraph.\n\nSecond paragraph.\n\n> Quoted text.\n\n-- \nSig";
        let result = extract_reply(text);
        assert_eq!(result, "First paragraph.\n\nSecond paragraph.");
    }

    #[test]
    fn test_extract_reply_windows_line_endings() {
        let text = "Content here.\r\n\r\n> Quoted.\r\n\r\n-- \r\nSig";
        let result = extract_reply(text);
        assert_eq!(result, "Content here.");
    }

    #[test]
    fn test_extract_reply_multiple_signatures_strip_from_first() {
        let text = "Content.\n\n-- \nFirst sig.\n\n-- \nSecond sig.";
        let result = extract_reply(text);
        assert_eq!(result, "Content.");
    }

    #[test]
    fn test_extract_reply_gt_in_content_not_stripped() {
        let text = "The salary > 100k is required.\nWe need x > y to hold.";
        let result = extract_reply(text);
        assert_eq!(
            result,
            "The salary > 100k is required.\nWe need x > y to hold."
        );
    }

    #[test]
    fn test_extract_reply_unicode_preserved() {
        let text = "こんにちは\n\n> 以前のメッセージ";
        let result = extract_reply(text);
        assert_eq!(result, "こんにちは");
    }

    #[test]
    fn test_extract_reply_empty_input() {
        assert_eq!(extract_reply(""), "");
    }

    #[test]
    fn test_extract_reply_em_dash_signature() {
        let text = "My message.\n\n\u{2014}\nName\nTitle";
        let result = extract_reply(text);
        assert_eq!(result, "My message.");
    }

    #[test]
    fn test_extract_reply_multiline_on_wrote_marker() {
        let text = "Sounds good!\n\nOn Monday, January 6, 2026 at 10:00 AM,\nJohn Doe <john@example.com> wrote:\n> original message";
        let result = extract_reply(text);
        assert_eq!(result, "Sounds good!");
    }

    #[test]
    fn test_extract_reply_quote_with_leading_whitespace() {
        let text = "Reply.\n\n  > Quoted with indent.";
        let result = extract_reply(text);
        assert_eq!(result, "Reply.");
    }
}
