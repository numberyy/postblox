use chrono::{DateTime, Utc};
use mail_parser::{Address, HeaderValue, MessageParser};

use crate::mail::error::MailError;

#[derive(Debug, Clone)]
pub struct ParsedEmail {
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    #[allow(dead_code)] // populated by parser, consumed when we add date-based sorting
    pub date: Option<DateTime<Utc>>,
    pub raw_headers: serde_json::Value,
}

pub fn parse(raw: &[u8]) -> Result<ParsedEmail, MailError> {
    let message = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| MailError::Parse("completely unparseable message".into()))?;

    let message_id = message.message_id().map(|s| strip_angles(s).to_string());

    let in_reply_to = extract_text_value(message.in_reply_to())
        .first()
        .map(|s| strip_angles(s).to_string());

    let references: Vec<String> = extract_text_value(message.references())
        .iter()
        .map(|s| strip_angles(s).to_string())
        .collect();

    let from = message
        .from()
        .and_then(|a| extract_addresses(a).into_iter().next())
        .unwrap_or_default();

    let to = message.to().map(extract_addresses).unwrap_or_default();

    let cc = message.cc().map(extract_addresses).unwrap_or_default();

    let subject = message.subject().map(|s| s.to_string());

    let text_body = message
        .body_text(0)
        .map(|s| s.into_owned())
        .filter(|s| !s.is_empty());

    let html_body = message
        .body_html(0)
        .map(|s| s.into_owned())
        .filter(|s| !s.is_empty());

    let date = message
        .date()
        .and_then(|dt| DateTime::from_timestamp(dt.to_timestamp(), 0));

    let raw_headers = build_raw_headers(&message);

    Ok(ParsedEmail {
        message_id,
        in_reply_to,
        references,
        from,
        to,
        cc,
        subject,
        text_body,
        html_body,
        date,
        raw_headers,
    })
}

fn strip_angles(s: &str) -> &str {
    s.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
}

fn extract_addresses(addr: &Address) -> Vec<String> {
    addr.iter()
        .filter_map(|a| a.address().map(|s| s.to_string()))
        .collect()
}

fn extract_text_value<'a>(value: &'a HeaderValue) -> Vec<&'a str> {
    match value {
        HeaderValue::Text(s) => vec![s.as_ref()],
        HeaderValue::TextList(list) => list.iter().map(|s| s.as_ref()).collect(),
        _ => vec![],
    }
}

fn build_raw_headers(message: &mail_parser::Message) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for header in message.headers() {
        let name = header.name().to_string();
        let value = header_value_to_json(header.value());
        match map.entry(name) {
            serde_json::map::Entry::Vacant(e) => {
                e.insert(value);
            }
            serde_json::map::Entry::Occupied(mut e) => {
                let existing = e.get_mut();
                if let serde_json::Value::Array(arr) = existing {
                    arr.push(value);
                } else {
                    let prev = existing.take();
                    *existing = serde_json::Value::Array(vec![prev, value]);
                }
            }
        }
    }
    serde_json::Value::Object(map)
}

fn header_value_to_json(value: &HeaderValue) -> serde_json::Value {
    match value {
        HeaderValue::Text(s) => serde_json::Value::String(s.to_string()),
        HeaderValue::TextList(list) => serde_json::Value::Array(
            list.iter()
                .map(|s| serde_json::Value::String(s.to_string()))
                .collect(),
        ),
        HeaderValue::Address(addr) => {
            let addrs = extract_addresses(addr);
            serde_json::Value::Array(addrs.into_iter().map(serde_json::Value::String).collect())
        }
        HeaderValue::DateTime(dt) => serde_json::Value::String(dt.to_rfc3339()),
        HeaderValue::ContentType(ct) => {
            let ctype = ct.ctype();
            match ct.subtype() {
                Some(sub) => serde_json::Value::String(format!("{ctype}/{sub}")),
                None => serde_json::Value::String(ctype.to_string()),
            }
        }
        HeaderValue::Empty => serde_json::Value::Null,
        other => serde_json::Value::String(format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("tests/fixtures/{name}")).expect("fixture file missing")
    }

    #[test]
    fn test_parse_simple_text_all_fields() {
        let email = parse(&fixture("simple_text.eml")).unwrap();
        assert_eq!(email.message_id.as_deref(), Some("msg001@example.com"));
        assert_eq!(email.from, "sender@example.com");
        assert_eq!(email.to, vec!["recipient@example.com"]);
        assert_eq!(email.subject.as_deref(), Some("Hello World"));
        assert!(email
            .text_body
            .as_ref()
            .unwrap()
            .contains("simple test email"));
        assert!(email.date.is_some());
        assert!(email.in_reply_to.is_none());
        assert!(email.references.is_empty());
        assert!(email.cc.is_empty());
    }

    #[test]
    fn test_parse_multipart_both_bodies() {
        let email = parse(&fixture("multipart.eml")).unwrap();
        assert!(email.text_body.is_some());
        assert!(email.html_body.is_some());
        assert!(email
            .text_body
            .as_ref()
            .unwrap()
            .contains("plain text version"));
        assert!(email.html_body.as_ref().unwrap().contains("<b>HTML</b>"));
    }

    #[test]
    fn test_parse_non_ascii_decoded() {
        let email = parse(&fixture("non_ascii.eml")).unwrap();
        // RFC 2047 encoded subject should be decoded by mail-parser
        assert!(email.subject.is_some());
        let subj = email.subject.as_ref().unwrap();
        assert!(
            subj.contains("テスト") || subj.contains("メール"),
            "subject should contain decoded Japanese: got {subj}"
        );
        assert!(email.text_body.as_ref().unwrap().contains("こんにちは"));
    }

    #[test]
    fn test_parse_malformed_returns_ok_with_partial_data() {
        let email = parse(&fixture("malformed.eml")).unwrap();
        assert_eq!(email.from, "broken@example.com");
        assert_eq!(
            email.message_id.as_deref(),
            Some("malformed001@example.com")
        );
        assert!(email.text_body.is_some());
    }

    #[test]
    fn test_parse_no_message_id() {
        let email = parse(&fixture("no_message_id.eml")).unwrap();
        assert!(email.message_id.is_none());
        assert_eq!(email.from, "noid@example.com");
        assert!(email.text_body.is_some());
    }

    #[test]
    fn test_parse_thread_chain_references() {
        let email = parse(&fixture("thread_chain.eml")).unwrap();
        assert_eq!(email.in_reply_to.as_deref(), Some("chain002@example.com"));
        assert_eq!(
            email.references,
            vec!["chain001@example.com", "chain002@example.com"]
        );
    }

    #[test]
    fn test_parse_empty_bytes_returns_error() {
        let result = parse(b"");
        assert!(matches!(result.unwrap_err(), MailError::Parse(_)));
    }

    #[test]
    fn test_parse_angle_bracket_stripping() {
        let raw = b"From: a@b.com\r\nMessage-ID: <abc@example.com>\r\n\r\nBody\r\n";
        let email = parse(raw).unwrap();
        assert_eq!(email.message_id.as_deref(), Some("abc@example.com"));
    }

    #[test]
    fn test_parse_message_id_without_angles() {
        // mail-parser may or may not include angles — strip_angles is idempotent
        let id = strip_angles("abc@example.com");
        assert_eq!(id, "abc@example.com");
        let id2 = strip_angles("<abc@example.com>");
        assert_eq!(id2, "abc@example.com");
    }

    #[test]
    fn test_parse_multiple_to_addresses() {
        let raw = b"From: x@y.com\r\nTo: a@x.com, b@x.com\r\n\r\nBody\r\n";
        let email = parse(raw).unwrap();
        assert_eq!(email.to, vec!["a@x.com", "b@x.com"]);
    }

    #[test]
    fn test_parse_from_with_display_name() {
        let raw = b"From: \"John Doe\" <john@example.com>\r\nTo: x@y.com\r\n\r\nBody\r\n";
        let email = parse(raw).unwrap();
        assert_eq!(email.from, "john@example.com");
    }

    #[test]
    fn test_parse_references_multiple_ids() {
        let email = parse(&fixture("nested_quotes.eml")).unwrap();
        assert_eq!(
            email.references,
            vec!["nested001@example.com", "nested002@example.com"]
        );
    }

    #[test]
    fn test_parse_empty_body_is_none() {
        let raw = b"From: a@b.com\r\nSubject: Empty\r\n\r\n";
        let email = parse(raw).unwrap();
        assert!(email.text_body.is_none());
    }

    #[test]
    fn test_parse_attachment_multipart_has_text() {
        let email = parse(&fixture("attachment_multipart.eml")).unwrap();
        assert!(email.text_body.as_ref().unwrap().contains("attached file"));
    }

    #[test]
    fn test_strip_angles_inner_whitespace() {
        assert_eq!(strip_angles("< abc@example.com >"), "abc@example.com");
        assert_eq!(strip_angles("<  spaced@ex.com  >"), "spaced@ex.com");
    }

    #[test]
    fn test_parse_raw_headers_populated() {
        let email = parse(&fixture("simple_text.eml")).unwrap();
        assert!(email.raw_headers.is_object());
        let obj = email.raw_headers.as_object().unwrap();
        assert!(obj.contains_key("From") || obj.contains_key("Subject"));
    }
}
