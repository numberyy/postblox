//! Reply / reply-all / forward draft construction. Pure functions:
//! given a [`MessageView`] plus the responding account's email, produce the
//! pre-filled headers, subject, and quoted body the composer should
//! show. RFC 5322 §3.6.4 controls `In-Reply-To` / `References`.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Maximum number of message-ids retained when chaining `References`.
/// Common practice is ~100; stop the chain growing without bound on
/// long mailing-list threads.
const REFERENCES_MESSAGE_ID_CAP: usize = 100;

/// Borrowed message data needed to prepare reply and forward drafts.
///
/// The daemon owns the full stored message model. This crate only needs
/// a framework-free, borrow-only view over the fields used to construct
/// mail drafts.
#[derive(Debug, Clone, Copy)]
pub struct MessageView<'a> {
    /// Stable identifier of the message being replied to or forwarded.
    pub id: Uuid,
    /// Address that appears in the original `From` header.
    pub from_addr: &'a str,
    /// Optional `Reply-To` override.
    pub reply_to: Option<&'a str>,
    /// Original subject, used to derive the draft subject.
    pub subject: Option<&'a str>,
    /// Original `Message-ID` header, used for `In-Reply-To`.
    pub message_id_header: Option<&'a str>,
    /// Original `References` header, extended for the draft.
    pub references_header: Option<&'a str>,
    /// Original `To` recipients, used for reply-all carry-over.
    pub to_addrs: &'a [String],
    /// Original `Cc` recipients, used for reply-all carry-over.
    pub cc_addrs: &'a [String],
    /// Plain-text body of the original, used to build the quoted block.
    pub text_body: Option<&'a str>,
    /// HTML body of the original, used only to detect HTML-only content.
    pub html_body: Option<&'a str>,
    /// Timestamp of the original, rendered into the quote attribution.
    pub internal_date: chrono::DateTime<chrono::Utc>,
}

/// Pre-filled headers + body the composer should populate when
/// replying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyDraft {
    /// Pre-filled recipients (the original sender, deduped).
    pub to: Vec<String>,
    /// Pre-filled `Cc` list when reply-all was requested.
    pub cc: Vec<String>,
    /// Subject line with a `Re: ` prefix added if absent.
    pub subject: String,
    /// Value to emit as the `In-Reply-To` header.
    pub in_reply_to: String,
    /// Value to emit as the `References` header.
    pub references: String,
    /// `> `-prefixed quote of the original plus attribution line.
    pub quoted_body: String,
}

/// Pre-filled state for a forward composer. `to` is intentionally empty
/// — forwards always require the user to pick a recipient. The
/// `forwarded_attachments` list refers back to the attachment rows on
/// the original message; the daemon turns each ref into bytes via
/// `attachment.fetch_for_forward`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardDraft {
    /// Recipients — always empty; the user must pick one.
    pub to: Vec<String>,
    /// Subject line with a `Fwd: ` prefix added if absent.
    pub subject: String,
    /// Forward block: divider, original headers, and original body.
    pub forwarded_body: String,
    /// References to the attachments carried over from the original.
    pub forwarded_attachments: Vec<ForwardAttachmentRef>,
}

/// Reference to one attachment on the message being forwarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardAttachmentRef {
    /// Identifier of the source message.
    pub message_id: Uuid,
    /// Identifier of the source attachment row.
    pub attachment_id: Uuid,
    /// Original filename of the attachment.
    pub filename: String,
    /// MIME type of the attachment as `type/subtype`.
    pub content_type: String,
    /// Stored size of the attachment in bytes.
    pub size_bytes: i64,
}

/// Build a reply (or reply-all) draft from a stored `Message`.
///
/// `account_email` is the address replying. When `reply_all` is true
/// the original `To` and `Cc` recipients are carried over to `Cc` with
/// the responding address removed and the reply target deduped.
pub fn reply_draft(message: MessageView<'_>, account_email: &str, reply_all: bool) -> ReplyDraft {
    let reply_target = primary_reply_target(message);
    let subject = re_prefix(message.subject.unwrap_or(""));
    let in_reply_to = message
        .message_id_header
        .map(angle_wrap)
        .unwrap_or_default();
    let references = build_references(message.references_header, message.message_id_header);

    let mut to: Vec<String> = if reply_target.is_empty() {
        Vec::new()
    } else {
        vec![reply_target.clone()]
    };
    let mut cc: Vec<String> = Vec::new();

    if reply_all {
        let mut seen_lower: Vec<String> = Vec::new();
        for addr in to.iter().chain([account_email.to_string()].iter()) {
            push_lower(&mut seen_lower, addr);
        }
        for addr in message.to_addrs.iter().chain(message.cc_addrs.iter()) {
            if !addr.trim().is_empty() && !contains_lower(&seen_lower, addr) {
                seen_lower.push(addr.to_ascii_lowercase());
                cc.push(addr.clone());
            }
        }
        if to.is_empty() {
            // Replying to a message with no `From` is a corner case but we
            // shouldn't drop the cc-extracted recipients.
            if let Some(first) = cc.first().cloned() {
                to.push(first);
                cc.remove(0);
            }
        }
    }

    let quoted_body = quote_body(message);

    ReplyDraft {
        to,
        cc,
        subject,
        in_reply_to,
        references,
        quoted_body,
    }
}

/// Build a forward draft for `message`. `forwarded_attachments`
/// references the original message's attachments by id so the caller
/// can re-fetch bytes (locally or via IMAP) before opening the
/// composer.
pub fn forward_draft(
    message: MessageView<'_>,
    attachments: &[(Uuid, String, String, i64)],
) -> ForwardDraft {
    let subject = fwd_prefix(message.subject.unwrap_or(""));
    let forwarded_body = forward_body(message);
    let forwarded_attachments = attachments
        .iter()
        .map(
            |(attachment_id, filename, content_type, size_bytes)| ForwardAttachmentRef {
                message_id: message.id,
                attachment_id: *attachment_id,
                filename: filename.clone(),
                content_type: content_type.clone(),
                size_bytes: *size_bytes,
            },
        )
        .collect();
    ForwardDraft {
        to: Vec::new(),
        subject,
        forwarded_body,
        forwarded_attachments,
    }
}

/// `Re: ` prefix that doesn't double-prefix. Detection is case
/// insensitive and tolerates a leading whitespace run, but it does NOT
/// strip prefixes from other clients (`Aw:`, `Fwd:`, `Re[2]:`) — the
/// goal is correctness, not normalisation.
pub fn re_prefix(subject: &str) -> String {
    if has_re_prefix(subject) {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    }
}

/// `Fwd: ` prefix that doesn't double-prefix. Same trade-offs as
/// [`re_prefix`].
pub fn fwd_prefix(subject: &str) -> String {
    if has_fwd_prefix(subject) {
        subject.to_string()
    } else {
        format!("Fwd: {subject}")
    }
}

fn has_re_prefix(subject: &str) -> bool {
    let trimmed = subject.trim_start();
    let mut chars = trimmed.chars();
    matches!(chars.next(), Some('R' | 'r'))
        && matches!(chars.next(), Some('E' | 'e'))
        && matches!(chars.next(), Some(':'))
}

fn has_fwd_prefix(subject: &str) -> bool {
    let trimmed = subject.trim_start();
    let lower: String = trimmed
        .chars()
        .take(4)
        .flat_map(char::to_lowercase)
        .collect();
    lower.starts_with("fwd:") || lower.starts_with("fw:")
}

fn primary_reply_target(message: MessageView<'_>) -> String {
    message
        .reply_to
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| message.from_addr.trim().to_string())
}

fn build_references(existing: Option<&str>, message_id: Option<&str>) -> String {
    let mut ids: Vec<String> = existing
        .map(|s| {
            s.split_whitespace()
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if let Some(mid) = message_id.map(str::trim).filter(|s| !s.is_empty()) {
        ids.push(angle_wrap(mid.to_string()));
    }
    if ids.len() > REFERENCES_MESSAGE_ID_CAP {
        // Keep the root and the most recent N-1 ids — the canonical
        // truncation rule on long threads.
        let tail_start = ids.len() - (REFERENCES_MESSAGE_ID_CAP - 1);
        let mut trimmed = Vec::with_capacity(REFERENCES_MESSAGE_ID_CAP);
        trimmed.push(ids[0].clone());
        trimmed.extend(ids.drain(tail_start..));
        ids = trimmed;
    }
    ids.join(" ")
}

fn angle_wrap(value: impl Into<String>) -> String {
    let s = value.into();
    let s = s.trim();
    if s.starts_with('<') && s.ends_with('>') {
        s.to_string()
    } else {
        format!("<{s}>")
    }
}

fn push_lower(seen: &mut Vec<String>, addr: &str) {
    let lower = addr.trim().to_ascii_lowercase();
    if !lower.is_empty() && !seen.iter().any(|s| s == &lower) {
        seen.push(lower);
    }
}

fn contains_lower(seen: &[String], addr: &str) -> bool {
    let lower = addr.trim().to_ascii_lowercase();
    seen.iter().any(|s| s == &lower)
}

/// Build the `> `-prefixed quote block + attribution line.
fn quote_body(message: MessageView<'_>) -> String {
    let attribution = format!(
        "On {}, {} wrote:",
        format_rfc2822(message.internal_date),
        message.from_addr
    );
    let mut out = String::new();
    out.push_str(&attribution);
    out.push_str("\r\n");
    out.push_str(&prefix_each_line(&body_for_quote(message), "> "));
    out
}

/// Forward body skeleton: a divider, the original headers, and the
/// original body (or a placeholder when only HTML was present).
fn forward_body(message: MessageView<'_>) -> String {
    let to_line = message.to_addrs.join(", ");
    let subject = message.subject.unwrap_or("");
    let mut out = String::new();
    out.push_str("---------- Forwarded message ----------\r\n");
    out.push_str(&format!("From: {}\r\n", message.from_addr));
    out.push_str(&format!(
        "Date: {}\r\n",
        format_rfc2822(message.internal_date)
    ));
    out.push_str(&format!("Subject: {subject}\r\n"));
    out.push_str(&format!("To: {to_line}\r\n"));
    out.push_str("\r\n");
    out.push_str(&body_for_quote(message));
    out
}

fn body_for_quote(message: MessageView<'_>) -> String {
    if let Some(text) = message.text_body.filter(|s| !s.is_empty()) {
        return text.replace("\r\n", "\n");
    }
    if message.html_body.map(|s| !s.is_empty()).unwrap_or(false) {
        return "[HTML body — quoted text unavailable]".to_string();
    }
    String::new()
}

fn prefix_each_line(text: &str, prefix: &str) -> String {
    if text.is_empty() {
        return prefix.to_string();
    }
    let mut out = String::with_capacity(text.len() + prefix.len() * 16);
    let mut first = true;
    for line in text.split('\n') {
        if !first {
            out.push_str("\r\n");
        }
        out.push_str(prefix);
        out.push_str(line);
        first = false;
    }
    out
}

fn format_rfc2822(dt: DateTime<Utc>) -> String {
    dt.format("%a, %d %b %Y %H:%M:%S %z").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    struct TestMessage {
        id: Uuid,
        from_addr: String,
        reply_to: Option<String>,
        subject: Option<String>,
        message_id_header: Option<String>,
        references_header: Option<String>,
        to_addrs: Vec<String>,
        cc_addrs: Vec<String>,
        text_body: Option<String>,
        html_body: Option<String>,
        internal_date: DateTime<Utc>,
    }

    impl TestMessage {
        fn view(&self) -> MessageView<'_> {
            MessageView {
                id: self.id,
                from_addr: &self.from_addr,
                reply_to: self.reply_to.as_deref(),
                subject: self.subject.as_deref(),
                message_id_header: self.message_id_header.as_deref(),
                references_header: self.references_header.as_deref(),
                to_addrs: &self.to_addrs,
                cc_addrs: &self.cc_addrs,
                text_body: self.text_body.as_deref(),
                html_body: self.html_body.as_deref(),
                internal_date: self.internal_date,
            }
        }
    }

    fn sample(subject: Option<&str>, from: &str) -> TestMessage {
        TestMessage {
            id: Uuid::new_v4(),
            message_id_header: Some("orig@example.com".into()),
            references_header: None,
            from_addr: from.into(),
            to_addrs: Vec::new(),
            cc_addrs: Vec::new(),
            reply_to: None,
            subject: subject.map(str::to_string),
            text_body: Some("Original line 1\nOriginal line 2".into()),
            html_body: None,
            internal_date: Utc.with_ymd_and_hms(2026, 5, 9, 10, 30, 0).unwrap(),
        }
    }

    #[test]
    fn test_re_prefix_adds_prefix_when_missing() {
        assert_eq!(re_prefix("foo"), "Re: foo");
    }

    #[test]
    fn test_re_prefix_does_not_double_prefix() {
        assert_eq!(re_prefix("Re: foo"), "Re: foo");
    }

    #[test]
    fn test_re_prefix_detection_is_case_insensitive() {
        assert_eq!(re_prefix("RE:foo"), "RE:foo");
        assert_eq!(re_prefix("re: foo"), "re: foo");
    }

    #[test]
    fn test_re_prefix_does_not_strip_other_prefixes() {
        assert_eq!(re_prefix("Fwd: foo"), "Re: Fwd: foo");
    }

    #[test]
    fn test_fwd_prefix_adds_prefix_when_missing() {
        assert_eq!(fwd_prefix("foo"), "Fwd: foo");
    }

    #[test]
    fn test_fwd_prefix_does_not_double_prefix() {
        assert_eq!(fwd_prefix("Fwd: foo"), "Fwd: foo");
        assert_eq!(fwd_prefix("FWD:foo"), "FWD:foo");
        assert_eq!(fwd_prefix("Fw: foo"), "Fw: foo");
    }

    #[test]
    fn test_reply_draft_in_reply_to_uses_message_id() {
        let msg = sample(Some("Hi"), "alice@x.com");
        let draft = reply_draft(msg.view(), "me@x.com", false);
        assert_eq!(draft.in_reply_to, "<orig@example.com>");
        assert_eq!(draft.references, "<orig@example.com>");
        assert_eq!(draft.subject, "Re: Hi");
        assert_eq!(draft.to, vec!["alice@x.com".to_string()]);
        assert!(draft.cc.is_empty());
    }

    #[test]
    fn test_reply_draft_appends_existing_references_chain() {
        let mut msg = sample(Some("Re: Hi"), "alice@x.com");
        msg.references_header = Some("<root@x>".into());
        msg.message_id_header = Some("orig@x".into());
        let draft = reply_draft(msg.view(), "me@x.com", false);
        assert_eq!(draft.references, "<root@x> <orig@x>");
        assert_eq!(draft.subject, "Re: Hi"); // no double prefix
    }

    #[test]
    fn test_reply_draft_handles_missing_message_id_gracefully() {
        let mut msg = sample(Some("Hi"), "alice@x.com");
        msg.message_id_header = None;
        let draft = reply_draft(msg.view(), "me@x.com", false);
        assert_eq!(draft.in_reply_to, "");
        assert_eq!(draft.references, "");
    }

    #[test]
    fn test_reply_draft_uses_reply_to_when_present() {
        let mut msg = sample(Some("Hi"), "alice@x.com");
        msg.reply_to = Some("alice-replies@x.com".into());
        let draft = reply_draft(msg.view(), "me@x.com", false);
        assert_eq!(draft.to, vec!["alice-replies@x.com".to_string()]);
    }

    #[test]
    fn test_reply_all_dedups_self_and_carries_others_to_cc() {
        let mut msg = sample(Some("Hi"), "alice@x.com");
        msg.to_addrs = vec!["a@x.com".into(), "b@x.com".into(), "Me@x.com".into()];
        msg.cc_addrs = vec!["c@x.com".into()];
        let draft = reply_draft(msg.view(), "me@x.com", true);
        assert_eq!(draft.to, vec!["alice@x.com".to_string()]);
        // me@x.com is dropped (case-insensitive). Original From and
        // every other recipient survives — alice is in To, so the
        // remaining To plus Cc form the new Cc list.
        assert_eq!(
            draft.cc,
            vec![
                "a@x.com".to_string(),
                "b@x.com".to_string(),
                "c@x.com".to_string(),
            ]
        );
    }

    #[test]
    fn test_reply_all_keeps_a_when_account_replies_to_itself() {
        // Self-reply: From == account_email. The reply should still go
        // to the original From per common convention; document that.
        let mut msg = sample(Some("Hi"), "a@x.com");
        msg.to_addrs = vec!["b@x.com".into()];
        let draft = reply_draft(msg.view(), "a@x.com", true);
        assert_eq!(draft.to, vec!["a@x.com".to_string()]);
        assert_eq!(draft.cc, vec!["b@x.com".to_string()]);
    }

    #[test]
    fn test_reply_all_dedup_is_case_insensitive() {
        let mut msg = sample(Some("Hi"), "alice@x.com");
        msg.to_addrs = vec!["B@X.COM".into(), "alice@x.com".into()];
        msg.cc_addrs = vec!["b@x.com".into()];
        let draft = reply_draft(msg.view(), "me@x.com", true);
        assert_eq!(draft.to, vec!["alice@x.com".to_string()]);
        assert_eq!(draft.cc, vec!["B@X.COM".to_string()]);
    }

    #[test]
    fn test_reply_quoted_body_contains_attribution_and_prefixed_lines() {
        let msg = sample(Some("Hi"), "alice@x.com");
        let draft = reply_draft(msg.view(), "me@x.com", false);
        assert!(
            draft
                .quoted_body
                .starts_with("On Sat, 09 May 2026 10:30:00 +0000, alice@x.com wrote:"),
            "got: {}",
            draft.quoted_body
        );
        assert!(draft.quoted_body.contains("> Original line 1"));
        assert!(draft.quoted_body.contains("> Original line 2"));
    }

    #[test]
    fn test_reply_quoted_body_html_only_falls_back_to_placeholder() {
        let mut msg = sample(Some("Hi"), "alice@x.com");
        msg.text_body = None;
        msg.html_body = Some("<p>Hi</p>".into());
        let draft = reply_draft(msg.view(), "me@x.com", false);
        assert!(draft.quoted_body.contains("> [HTML body"));
    }

    #[test]
    fn test_forward_draft_subject_and_body_shape() {
        let msg = sample(Some("Subject"), "alice@x.com");
        let draft = forward_draft(msg.view(), &[]);
        assert_eq!(draft.subject, "Fwd: Subject");
        assert!(draft.to.is_empty());
        assert!(draft
            .forwarded_body
            .contains("---------- Forwarded message ----------"));
        assert!(draft.forwarded_body.contains("From: alice@x.com"));
        assert!(draft.forwarded_body.contains("Subject: Subject"));
    }

    #[test]
    fn test_forward_draft_attachments_carry_message_id() {
        let msg = sample(Some("S"), "alice@x.com");
        let attachment_id = Uuid::new_v4();
        let draft = forward_draft(
            msg.view(),
            &[(
                attachment_id,
                "report.pdf".to_string(),
                "application/pdf".to_string(),
                123,
            )],
        );
        assert_eq!(draft.forwarded_attachments.len(), 1);
        let a = &draft.forwarded_attachments[0];
        assert_eq!(a.message_id, msg.id);
        assert_eq!(a.attachment_id, attachment_id);
        assert_eq!(a.filename, "report.pdf");
        assert_eq!(a.size_bytes, 123);
    }

    #[test]
    fn test_references_chain_caps_length_keeping_root_and_recent() {
        let mut existing = String::new();
        for i in 0..200 {
            existing.push_str(&format!("<id{i}@x> "));
        }
        let chain = build_references(Some(&existing), Some("latest@x"));
        let parts: Vec<&str> = chain.split_whitespace().collect();
        assert_eq!(parts.len(), REFERENCES_MESSAGE_ID_CAP);
        assert_eq!(parts.first().copied(), Some("<id0@x>"));
        assert_eq!(parts.last().copied(), Some("<latest@x>"));
    }

    #[test]
    fn test_references_chain_handles_missing_existing() {
        assert_eq!(build_references(None, Some("only@x")), "<only@x>");
    }
}
