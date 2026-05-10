//! Integration tests for the framework-free `postblox-mail` crate.
//!
//! These tests cross module boundaries (parse → thread → reply → re-build)
//! to exercise the public API the way the rest of the crate uses it. The
//! inline `#[cfg(test)] mod tests` blocks under `src/*` cover each
//! module in isolation; this file covers the seams.
//!
//! See rust-skills audit finding F-L2 in `plans/rust-skills-review.md`.

use chrono::{TimeZone, Utc};
use uuid::Uuid;

use postblox_mail::builder::{build_mime_full, MimeAttachment, MimeBuildOptions, ReplyHeaders};
use postblox_mail::parser::{parse, Disposition};
use postblox_mail::reply::{forward_draft, reply_draft, MessageView};
use postblox_mail::threading::{assign_thread, ThreadMatch, ThreadRef};

const SIMPLE_TEXT_EMAIL: &[u8] = b"From: alice@example.com\r\n\
To: bob@example.com\r\n\
Cc: carol@example.com\r\n\
Subject: Quarterly report\r\n\
Date: Mon, 1 May 2026 10:00:00 +0000\r\n\
Message-ID: <abc123@example.com>\r\n\
\r\n\
Hi Bob,\r\n\
\r\n\
Numbers attached next time.\r\n\
\r\n\
-- Alice\r\n";

// 4-byte payload "test" base64-encoded -> "dGVzdA==".
const MULTIPART_WITH_ATTACHMENT: &[u8] = b"From: dana@example.com\r\n\
To: erin@example.com\r\n\
Subject: Logs for review\r\n\
Date: Mon, 1 May 2026 10:00:00 +0000\r\n\
Message-ID: <multi001@example.com>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"BOUND\"\r\n\
\r\n\
--BOUND\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
See attached log file.\r\n\
--BOUND\r\n\
Content-Type: text/plain; charset=us-ascii; name=\"snippet.log\"\r\n\
Content-Disposition: attachment; filename=\"snippet.log\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
dGVzdA==\r\n\
--BOUND--\r\n";

const REPLY_TO_THREAD_ROOT: &[u8] = b"From: alice@example.com\r\n\
To: bob@example.com\r\n\
Subject: Re: Quarterly report\r\n\
Date: Mon, 1 May 2026 11:00:00 +0000\r\n\
Message-ID: <reply999@example.com>\r\n\
In-Reply-To: <abc123@example.com>\r\n\
References: <abc123@example.com>\r\n\
\r\n\
Acknowledged.\r\n";

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
    internal_date: chrono::DateTime<Utc>,
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

fn sample_message_for_reply() -> TestMessage {
    TestMessage {
        id: Uuid::new_v4(),
        message_id_header: Some("orig@example.com".into()),
        references_header: Some("<root@example.com>".into()),
        from_addr: "alice@example.com".into(),
        to_addrs: vec!["bob@example.com".into()],
        cc_addrs: Vec::new(),
        reply_to: None,
        subject: Some("Quarterly report".into()),
        text_body: Some("Original body line 1\nOriginal body line 2".into()),
        html_body: None,
        internal_date: Utc.with_ymd_and_hms(2026, 5, 1, 10, 0, 0).unwrap(),
    }
}

#[test]
fn test_parse_simple_text_email_extracts_from_subject_and_body() {
    let email = parse(SIMPLE_TEXT_EMAIL).expect("simple text email should parse");
    assert_eq!(email.from, "alice@example.com");
    assert_eq!(email.to, vec!["bob@example.com"]);
    assert_eq!(email.cc, vec!["carol@example.com"]);
    assert_eq!(email.subject.as_deref(), Some("Quarterly report"));
    assert_eq!(email.message_id.as_deref(), Some("abc123@example.com"));
    assert!(
        email
            .text_body
            .as_deref()
            .expect("text body present")
            .contains("Numbers attached"),
        "body should contain expected snippet"
    );
    assert!(
        email.attachments.is_empty(),
        "single-part text email has no attachments"
    );
}

#[test]
fn test_parse_multipart_with_attachment_returns_filename_and_size() {
    let email = parse(MULTIPART_WITH_ATTACHMENT).expect("multipart email should parse");
    assert_eq!(
        email.attachments.len(),
        1,
        "expected exactly one attachment"
    );
    let att = &email.attachments[0];
    assert_eq!(att.filename, "snippet.log");
    assert_eq!(att.content_type, "text/plain");
    assert_eq!(att.disposition, Disposition::Attachment);
    assert_eq!(att.data, b"test");
    assert!(
        email
            .text_body
            .as_deref()
            .expect("multipart text body")
            .contains("See attached log file"),
        "text body should be preserved alongside the attachment"
    );
}

#[test]
fn test_assign_thread_with_no_match_returns_new() {
    let parsed = parse(SIMPLE_TEXT_EMAIL).expect("parse simple email");
    let other = ThreadRef {
        thread_id: Uuid::new_v4(),
        message_ids: vec!["unrelated@example.com".into()],
        subject: "Wholly different topic".into(),
        last_message_at: Utc::now(),
    };
    match assign_thread(&parsed, &[other]) {
        ThreadMatch::New => {}
        ThreadMatch::Existing(_) => panic!("fresh subject + no header match must start new thread"),
    }
}

#[test]
fn test_assign_thread_with_in_reply_to_match_returns_existing_id() {
    let parsed = parse(REPLY_TO_THREAD_ROOT).expect("reply email should parse");
    let target_id = Uuid::new_v4();
    let threads = vec![ThreadRef {
        thread_id: target_id,
        message_ids: vec!["abc123@example.com".into()],
        subject: "Quarterly report".into(),
        last_message_at: Utc::now(),
    }];
    match assign_thread(&parsed, &threads) {
        ThreadMatch::Existing(id) => assert_eq!(id, target_id),
        ThreadMatch::New => panic!("In-Reply-To match should return Existing"),
    }
}

#[test]
fn test_assign_thread_subject_match_within_cutoff_returns_existing() {
    let parsed = parse(SIMPLE_TEXT_EMAIL).expect("parse email");
    let target_id = Uuid::new_v4();
    // Subject matches after normalising "Re:"; thread is fresh (1 day old).
    let threads = vec![ThreadRef {
        thread_id: target_id,
        message_ids: vec!["unrelated@example.com".into()],
        subject: "Re: Quarterly report".into(),
        last_message_at: Utc::now() - chrono::Duration::days(1),
    }];
    match assign_thread(&parsed, &threads) {
        ThreadMatch::Existing(id) => assert_eq!(id, target_id),
        ThreadMatch::New => panic!("matching subject within cutoff should return Existing"),
    }
}

#[test]
fn test_assign_thread_subject_match_beyond_cutoff_returns_new() {
    let parsed = parse(SIMPLE_TEXT_EMAIL).expect("parse email");
    let stale_thread = ThreadRef {
        thread_id: Uuid::new_v4(),
        message_ids: vec!["unrelated@example.com".into()],
        subject: "Quarterly report".into(),
        last_message_at: Utc::now() - chrono::Duration::days(30),
    };
    match assign_thread(&parsed, &[stale_thread]) {
        ThreadMatch::New => {}
        ThreadMatch::Existing(_) => panic!("stale subject match must NOT join existing thread"),
    }
}

#[test]
fn test_build_reply_emits_in_reply_to_and_references() {
    let original = sample_message_for_reply();
    let draft = reply_draft(original.view(), "me@example.com", false);

    assert_eq!(draft.subject, "Re: Quarterly report");
    assert_eq!(draft.in_reply_to, "<orig@example.com>");
    // References chain = existing references + this message's id.
    assert_eq!(draft.references, "<root@example.com> <orig@example.com>");
    assert!(
        draft.quoted_body.contains("> Original body line 1"),
        "reply quote should prefix lines with '> '"
    );

    // Round-trip the draft headers through the MIME builder and confirm
    // the threading headers land on the wire.
    let raw = build_mime_full(MimeBuildOptions {
        from: "me@example.com",
        to: &draft.to,
        cc: &draft.cc,
        subject: &draft.subject,
        text_body: Some("Acknowledged.\r\n"),
        html_body: None,
        message_id: "<my-reply@postblox>",
        attachments: &[],
        reply: ReplyHeaders {
            in_reply_to: Some(&draft.in_reply_to),
            references: Some(&draft.references),
        },
    });
    let s = String::from_utf8(raw).expect("MIME output should be valid UTF-8");
    assert!(
        s.contains("In-Reply-To: <orig@example.com>\r\n"),
        "MIME output missing In-Reply-To header"
    );
    assert!(
        s.contains("References: <root@example.com> <orig@example.com>\r\n"),
        "MIME output missing References header"
    );
    assert!(
        s.contains("Subject: Re: Quarterly report\r\n"),
        "MIME output missing reply subject"
    );
}

#[test]
fn test_build_forward_does_not_emit_in_reply_to() {
    let original = sample_message_for_reply();
    let draft = forward_draft(original.view(), &[]);
    assert_eq!(draft.subject, "Fwd: Quarterly report");
    assert!(draft.to.is_empty(), "forward composer leaves To empty");
    assert!(
        draft
            .forwarded_body
            .contains("---------- Forwarded message ----------"),
        "forward body must include divider"
    );
    assert!(
        draft.forwarded_body.contains("From: alice@example.com"),
        "forward body must include original From"
    );
    assert!(
        draft.forwarded_body.contains("Original body line 1"),
        "forward body must include original text"
    );

    // Build a MIME message for the forward; explicitly no ReplyHeaders.
    let raw = build_mime_full(MimeBuildOptions {
        from: "me@example.com",
        to: &["new-recipient@example.com".to_string()],
        cc: &[],
        subject: &draft.subject,
        text_body: Some(&draft.forwarded_body),
        html_body: None,
        message_id: "<my-fwd@postblox>",
        attachments: &[],
        reply: ReplyHeaders::default(),
    });
    let s = String::from_utf8(raw).expect("MIME output should be valid UTF-8");
    assert!(
        !s.contains("In-Reply-To"),
        "forward MIME must not emit In-Reply-To"
    );
    assert!(
        !s.contains("References:"),
        "forward MIME must not emit References"
    );
    assert!(
        s.contains("Subject: Fwd: Quarterly report\r\n"),
        "forward MIME missing forward subject"
    );
}

#[test]
fn test_parse_then_build_round_trip_preserves_attachment_bytes() {
    // Cross-module flow: parse a message with an attachment, then rebuild
    // an outgoing message that re-attaches the same bytes; reparse and
    // confirm the payload survives the round trip.
    let parsed = parse(MULTIPART_WITH_ATTACHMENT).expect("parse multipart");
    let original = &parsed.attachments[0];

    let raw = build_mime_full(MimeBuildOptions {
        from: "me@example.com",
        to: &["other@example.com".to_string()],
        cc: &[],
        subject: "Forwarding logs",
        text_body: Some("See attached"),
        html_body: None,
        message_id: "<roundtrip@postblox>",
        attachments: &[MimeAttachment {
            filename: original.filename.clone(),
            content_type: original.content_type.clone(),
            data: original.data.clone(),
            content_id: None,
        }],
        reply: ReplyHeaders::default(),
    });

    let reparsed = parse(&raw).expect("rebuilt MIME should parse");
    assert_eq!(
        reparsed.attachments.len(),
        1,
        "rebuilt message should have one attachment"
    );
    let rt = &reparsed.attachments[0];
    assert_eq!(rt.filename, "snippet.log");
    assert_eq!(rt.data, b"test");
    assert_eq!(rt.disposition, Disposition::Attachment);
}
