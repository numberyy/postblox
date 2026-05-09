//! RFC 5322 / MIME parsing — raw bytes in, [`ParsedEmail`] out.
//!
//! Single entry point: [`parse`]. It pulls headers, normalises
//! recipient lists, picks `text/plain` and `text/html` body parts, and
//! collects attachments into [`ParsedAttachment`] with a
//! [`Disposition`] tag.
//!
//! This is the bench-gate hot path: [`parse`] runs on every inbound
//! IMAP message and CLAUDE.md targets ≥ 5,000 msgs/sec for parsing
//! throughput. Underlying dep is `mail-parser` (Tier 2 in CLAUDE.md —
//! wrapped here so the rest of the crate doesn't import it directly).
//!
//! Failures land in [`crate::mail::error::MailError::Parse`].

use mail_parser::{Address, HeaderValue, MessageParser, MimeHeaders, PartType};
use serde::{Deserialize, Serialize};

use crate::mail::error::MailError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Inline,
    Attachment,
}

#[derive(Debug, Clone)]
pub struct ParsedAttachment {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
    pub disposition: Disposition,
    pub content_id: Option<String>,
}

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
    pub raw_headers: serde_json::Value,
    pub attachments: Vec<ParsedAttachment>,
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
        .and_then(|a| a.iter().find_map(|x| x.address().map(str::to_string)))
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

    let raw_headers = build_raw_headers(&message);

    let mut attachments = Vec::with_capacity(message.parts.len().min(8));
    for (i, part) in message.parts.iter().enumerate() {
        let disposition = part.content_disposition();
        let disposition_type = disposition.map(|d| d.ctype());
        let is_attachment = disposition_type == Some("attachment");
        let is_inline = disposition_type == Some("inline");

        let has_filename = part.attachment_name().is_some();

        if !is_attachment && !is_inline && !has_filename {
            continue;
        }

        // Skip text/html and text/plain body parts (index 0 is the root)
        if i == 0 {
            continue;
        }
        match &part.body {
            PartType::Text(_) | PartType::Html(_) if !is_attachment && !has_filename => continue,
            PartType::Multipart(_) | PartType::Message(_) => continue,
            _ => {}
        }

        let data = match &part.body {
            PartType::Binary(cow) | PartType::InlineBinary(cow) => cow.to_vec(),
            PartType::Text(cow) => cow.as_bytes().to_vec(),
            PartType::Html(cow) => cow.as_bytes().to_vec(),
            _ => continue,
        };

        let content_type = part
            .content_type()
            .map(|ct| {
                let ctype = ct.ctype();
                match ct.subtype() {
                    Some(sub) => {
                        let mut s = String::with_capacity(ctype.len() + 1 + sub.len());
                        s.push_str(ctype);
                        s.push('/');
                        s.push_str(sub);
                        s
                    }
                    None => ctype.to_string(),
                }
            })
            .unwrap_or_else(|| "application/octet-stream".into());

        let filename = part
            .attachment_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                let ext = mime_to_ext(&content_type);
                format!("attachment_{i}.{ext}")
            });

        let disposition = if is_inline {
            Disposition::Inline
        } else {
            Disposition::Attachment
        };

        let content_id = part.content_id().map(|s| strip_angles(s).to_string());

        attachments.push(ParsedAttachment {
            filename,
            content_type,
            data,
            disposition,
            content_id,
        });
    }

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
        raw_headers,
        attachments,
    })
}

fn mime_to_ext(content_type: &str) -> &'static str {
    match content_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "text/html" => "html",
        "text/csv" => "csv",
        "application/json" => "json",
        "application/zip" => "zip",
        "application/gzip" => "gz",
        _ => "bin",
    }
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
                Some(sub) => {
                    let mut s = String::with_capacity(ctype.len() + 1 + sub.len());
                    s.push_str(ctype);
                    s.push('/');
                    s.push_str(sub);
                    serde_json::Value::String(s)
                }
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
    fn test_parse_attachment_multipart_extracts_attachment() {
        let email = parse(&fixture("attachment_multipart.eml")).unwrap();
        assert_eq!(email.attachments.len(), 1);
        let att = &email.attachments[0];
        assert_eq!(att.filename, "data.bin");
        assert_eq!(att.content_type, "application/octet-stream");
        assert_eq!(att.disposition, Disposition::Attachment);
        assert_eq!(att.data, b"Hello World");
    }

    #[test]
    fn test_parse_simple_text_no_attachments() {
        let email = parse(&fixture("simple_text.eml")).unwrap();
        assert!(email.attachments.is_empty());
    }

    #[test]
    fn test_parse_multipart_no_spurious_attachments() {
        let email = parse(&fixture("multipart.eml")).unwrap();
        assert!(
            email.attachments.is_empty(),
            "text/plain + text/html bodies should not be treated as attachments"
        );
    }

    #[test]
    fn test_parse_inline_attachment() {
        let raw = b"From: a@b.com\r\nTo: x@y.com\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"bound\"\r\n\r\n\
--bound\r\nContent-Type: text/plain\r\n\r\nHello\r\n\
--bound\r\nContent-Type: image/png\r\nContent-Disposition: inline; filename=\"logo.png\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n\
--bound--\r\n";
        let email = parse(raw).unwrap();
        assert_eq!(email.attachments.len(), 1);
        assert_eq!(email.attachments[0].filename, "logo.png");
        assert_eq!(email.attachments[0].disposition, Disposition::Inline);
    }

    #[test]
    fn test_parse_attachment_missing_filename_generates_name() {
        let raw = b"From: a@b.com\r\nTo: x@y.com\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"bound\"\r\n\r\n\
--bound\r\nContent-Type: text/plain\r\n\r\nHello\r\n\
--bound\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment\r\n\
Content-Transfer-Encoding: base64\r\n\r\nJVBERi0=\r\n\
--bound--\r\n";
        let email = parse(raw).unwrap();
        assert_eq!(email.attachments.len(), 1);
        assert!(
            email.attachments[0].filename.starts_with("attachment_"),
            "generated filename should start with attachment_"
        );
        assert!(
            email.attachments[0].filename.ends_with(".pdf"),
            "generated filename should end with .pdf for application/pdf"
        );
    }

    #[test]
    fn test_mime_to_ext_known_types() {
        assert_eq!(mime_to_ext("image/png"), "png");
        assert_eq!(mime_to_ext("image/jpeg"), "jpg");
        assert_eq!(mime_to_ext("application/pdf"), "pdf");
        assert_eq!(mime_to_ext("text/plain"), "txt");
        assert_eq!(mime_to_ext("application/json"), "json");
    }

    #[test]
    fn test_mime_to_ext_unknown_type() {
        assert_eq!(mime_to_ext("application/x-custom"), "bin");
    }

    #[test]
    fn test_parse_cid_attachment_extracts_content_id() {
        let raw = b"From: a@b.com\r\nTo: x@y.com\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/related; boundary=\"rel\"\r\n\r\n\
--rel\r\nContent-Type: text/html\r\n\r\n<html><img src=\"cid:logo123@example.com\"></html>\r\n\
--rel\r\nContent-Type: image/png\r\nContent-Disposition: inline; filename=\"logo.png\"\r\n\
Content-ID: <logo123@example.com>\r\n\
Content-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n\
--rel--\r\n";
        let email = parse(raw).unwrap();
        assert_eq!(email.attachments.len(), 1);
        assert_eq!(
            email.attachments[0].content_id.as_deref(),
            Some("logo123@example.com")
        );
        assert_eq!(email.attachments[0].disposition, Disposition::Inline);
    }

    #[test]
    fn test_parse_attachment_without_cid_has_none() {
        let email = parse(&fixture("attachment_multipart.eml")).unwrap();
        assert_eq!(email.attachments.len(), 1);
        assert!(email.attachments[0].content_id.is_none());
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
