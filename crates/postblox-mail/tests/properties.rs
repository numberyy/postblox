use postblox_mail::builder::{build_mime_full, MimeAttachment, MimeBuildOptions, ReplyHeaders};
use postblox_mail::parser::{parse_with_options, ParseOptions};
use postblox_mail::reply_extract::extract_reply;
use postblox_mail::threading::normalize_subject;
use proptest::prelude::*;
use proptest::test_runner::Config as ProptestConfig;

fn printable_line() -> impl Strategy<Value = String> {
    "[A-Za-z0-9 .,!?_@<>-]{0,48}"
}

fn reply_text() -> impl Strategy<Value = String> {
    prop::collection::vec(printable_line(), 0..8).prop_map(|lines| lines.join("\r\n"))
}

fn subject_with_prefixes() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(
            prop_oneof![Just("Re:"), Just("RE:"), Just("Fwd:"), Just("FW:")],
            0..6,
        ),
        "[A-Za-z0-9 .,!?_-]{0,64}",
    )
        .prop_map(|(prefixes, subject)| {
            let mut value = String::new();
            for prefix in prefixes {
                value.push_str(prefix);
                value.push(' ');
            }
            value.push_str(&subject);
            value
        })
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn normalize_subject_is_idempotent(subject in subject_with_prefixes()) {
        let once = normalize_subject(&subject).into_owned();
        let twice = normalize_subject(&once).into_owned();
        prop_assert_eq!(once, twice);
    }

    #[test]
    fn extract_reply_is_idempotent_and_removes_quote_lines(text in reply_text()) {
        let once = extract_reply(&text);
        let twice = extract_reply(&once);

        prop_assert_eq!(&once, &twice);
        prop_assert!(!once.contains('\r'));
        prop_assert!(!once.lines().any(|line| line.trim_start().starts_with('>')));
    }

    #[test]
    fn build_then_parse_mime_preserves_attachment(
        subject in "[A-Za-z0-9][A-Za-z0-9.,!?_-]{0,47}",
        body in "[A-Za-z0-9][A-Za-z0-9 .,!?_@<>-]{0,95}",
        filename in "[a-z]{1,16}\\.bin",
        data in prop::collection::vec(any::<u8>(), 1..128),
    ) {
        let to = vec!["bob@example.com".to_string()];
        let attachment = MimeAttachment {
            filename: filename.clone(),
            content_type: "application/octet-stream".into(),
            data: data.clone(),
            content_id: None,
        };
        let raw = build_mime_full(MimeBuildOptions {
            from: "alice@example.com",
            to: &to,
            cc: &[],
            subject: &subject,
            text_body: Some(&body),
            html_body: None,
            message_id: "<prop@example.com>",
            attachments: &[attachment],
            reply: ReplyHeaders::default(),
        });

        let parsed = parse_with_options(&raw, ParseOptions::without_raw_headers())?;

        prop_assert_eq!(parsed.from, "alice@example.com");
        prop_assert_eq!(parsed.to, to);
        prop_assert_eq!(parsed.subject.as_deref(), Some(subject.as_str()));
        prop_assert_eq!(parsed.text_body.as_deref(), Some(body.as_str()));
        prop_assert_eq!(parsed.attachments.len(), 1);
        prop_assert_eq!(&parsed.attachments[0].filename, &filename);
        prop_assert_eq!(&parsed.attachments[0].data, &data);
    }
}
