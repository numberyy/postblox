use std::fmt::Write;

use chrono::Utc;
use uuid::Uuid;

use base64::Engine;

/// Sanitize a header value by stripping CRLF to prevent header injection.
fn sanitize_header(value: &str) -> String {
    value.replace(['\r', '\n'], "")
}

pub fn build_mime(
    from: &str,
    to: &[String],
    cc: &[String],
    subject: &str,
    text_body: Option<&str>,
    html_body: Option<&str>,
    message_id: &str,
) -> Vec<u8> {
    build_mime_with_attachments(from, to, cc, subject, text_body, html_body, message_id, &[])
}

pub struct MimeAttachment {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
pub fn build_mime_with_attachments(
    from: &str,
    to: &[String],
    cc: &[String],
    subject: &str,
    text_body: Option<&str>,
    html_body: Option<&str>,
    message_id: &str,
    attachments: &[MimeAttachment],
) -> Vec<u8> {
    let mut msg = String::new();
    // write! to String is infallible — fmt::Write for String never returns Err.
    let _ = write!(msg, "From: {}\r\n", sanitize_header(from));
    let _ = write!(msg, "To: {}\r\n", sanitize_header(&to.join(", ")));
    if !cc.is_empty() {
        let _ = write!(msg, "Cc: {}\r\n", sanitize_header(&cc.join(", ")));
    }
    let _ = write!(msg, "Subject: {}\r\n", sanitize_header(subject));
    let _ = write!(msg, "Message-ID: {}\r\n", sanitize_header(message_id));
    let _ = write!(
        msg,
        "Date: {}\r\n",
        Utc::now().format("%a, %d %b %Y %H:%M:%S +0000")
    );
    msg.push_str("MIME-Version: 1.0\r\n");

    if attachments.is_empty() {
        build_body_only(&mut msg, text_body, html_body);
    } else {
        let mixed_boundary = format!("postblox-mixed-{}", Uuid::new_v4().simple());
        let _ = write!(
            msg,
            "Content-Type: multipart/mixed; boundary=\"{mixed_boundary}\"\r\n"
        );
        msg.push_str("\r\n");

        let _ = write!(msg, "--{mixed_boundary}\r\n");
        build_body_only(&mut msg, text_body, html_body);

        for att in attachments {
            let _ = write!(msg, "\r\n--{mixed_boundary}\r\n");
            let _ = write!(
                msg,
                "Content-Type: {}; name=\"{}\"\r\n",
                sanitize_header(&att.content_type),
                sanitize_header(&att.filename)
            );
            msg.push_str("Content-Transfer-Encoding: base64\r\n");
            let _ = write!(
                msg,
                "Content-Disposition: attachment; filename=\"{}\"\r\n",
                sanitize_header(&att.filename)
            );
            msg.push_str("\r\n");
            let encoded = base64::engine::general_purpose::STANDARD.encode(&att.data);
            // Line-wrap at 76 chars per RFC 2045
            for chunk in encoded.as_bytes().chunks(76) {
                // base64 Standard encoding always produces valid ASCII
                msg.push_str(std::str::from_utf8(chunk).expect("base64 is always valid ASCII"));
                msg.push_str("\r\n");
            }
        }
        let _ = write!(msg, "--{mixed_boundary}--\r\n");
    }

    msg.into_bytes()
}

fn build_body_only(msg: &mut String, text_body: Option<&str>, html_body: Option<&str>) {
    match (text_body, html_body) {
        (Some(text), Some(html)) => {
            let boundary = format!("postblox-{}", Uuid::new_v4().simple());
            let _ = write!(
                msg,
                "Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n"
            );
            msg.push_str("\r\n");
            let _ = write!(msg, "--{boundary}\r\n");
            msg.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
            msg.push_str(text);
            let _ = write!(msg, "\r\n--{boundary}\r\n");
            msg.push_str("Content-Type: text/html; charset=utf-8\r\n\r\n");
            msg.push_str(html);
            let _ = write!(msg, "\r\n--{boundary}--\r\n");
        }
        (Some(text), None) => {
            msg.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
            msg.push_str(text);
        }
        (None, Some(html)) => {
            msg.push_str("Content-Type: text/html; charset=utf-8\r\n\r\n");
            msg.push_str(html);
        }
        (None, None) => {
            msg.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_mime_text_only() {
        let s = String::from_utf8(build_mime(
            "bot@example.com",
            &["user@example.com".into()],
            &[],
            "Hello",
            Some("Body text"),
            None,
            "<msg@postblox>",
        ))
        .unwrap();
        assert!(s.contains("From: bot@example.com\r\n"));
        assert!(s.contains("To: user@example.com\r\n"));
        assert!(s.contains("Subject: Hello\r\n"));
        assert!(s.contains("Content-Type: text/plain; charset=utf-8"));
        assert!(s.contains("Body text"));
    }

    #[test]
    fn test_build_mime_both_multipart() {
        let s = String::from_utf8(build_mime(
            "bot@example.com",
            &["user@example.com".into()],
            &[],
            "Both",
            Some("Plain"),
            Some("<b>HTML</b>"),
            "<msg@postblox>",
        ))
        .unwrap();
        assert!(s.contains("multipart/alternative"));
        assert!(s.contains("Plain"));
        assert!(s.contains("<b>HTML</b>"));
    }

    #[test]
    fn test_build_mime_html_only() {
        let s = String::from_utf8(build_mime(
            "bot@example.com",
            &["user@example.com".into()],
            &[],
            "HTML",
            None,
            Some("<b>Bold</b>"),
            "<msg@postblox>",
        ))
        .unwrap();
        assert!(s.contains("Content-Type: text/html; charset=utf-8"));
        assert!(s.contains("<b>Bold</b>"));
    }

    #[test]
    fn test_build_mime_with_cc() {
        let s = String::from_utf8(build_mime(
            "bot@example.com",
            &["a@example.com".into()],
            &["cc@example.com".into()],
            "CC",
            Some("Body"),
            None,
            "<msg@postblox>",
        ))
        .unwrap();
        assert!(s.contains("Cc: cc@example.com\r\n"));
    }

    #[test]
    fn test_build_mime_header_injection_stripped() {
        let s = String::from_utf8(build_mime(
            "bot@example.com",
            &["user@example.com".into()],
            &[],
            "Injected\r\nBcc: attacker@evil.com",
            Some("Body"),
            None,
            "<msg@postblox>",
        ))
        .unwrap();
        // CRLF stripped — no separate Bcc header injected.
        assert!(!s.contains("\r\nBcc:"));
        assert!(s.contains("Subject: InjectedBcc: attacker@evil.com\r\n"));
    }

    #[test]
    fn test_build_mime_with_attachments_multipart_mixed() {
        let attachments = vec![MimeAttachment {
            filename: "report.pdf".into(),
            content_type: "application/pdf".into(),
            data: b"PDF content here".to_vec(),
        }];
        let s = String::from_utf8(build_mime_with_attachments(
            "bot@example.com",
            &["user@example.com".into()],
            &[],
            "With Attachment",
            Some("Body text"),
            None,
            "<msg@postblox>",
            &attachments,
        ))
        .unwrap();
        assert!(s.contains("multipart/mixed"));
        assert!(s.contains("Body text"));
        assert!(s.contains("Content-Disposition: attachment; filename=\"report.pdf\""));
        assert!(s.contains("Content-Transfer-Encoding: base64"));
    }

    #[test]
    fn test_build_mime_with_attachments_contains_base64_data() {
        let data = b"hello world attachment data";
        let attachments = vec![MimeAttachment {
            filename: "test.txt".into(),
            content_type: "text/plain".into(),
            data: data.to_vec(),
        }];
        let s = String::from_utf8(build_mime_with_attachments(
            "bot@example.com",
            &["user@example.com".into()],
            &[],
            "Test",
            Some("Body"),
            None,
            "<msg@postblox>",
            &attachments,
        ))
        .unwrap();
        let expected_b64 = base64::engine::general_purpose::STANDARD.encode(data);
        assert!(s.contains(&expected_b64));
    }

    #[test]
    fn test_build_mime_with_attachments_and_html() {
        let attachments = vec![MimeAttachment {
            filename: "img.png".into(),
            content_type: "image/png".into(),
            data: vec![0x89, 0x50, 0x4E, 0x47],
        }];
        let s = String::from_utf8(build_mime_with_attachments(
            "bot@example.com",
            &["user@example.com".into()],
            &[],
            "Mixed",
            Some("Text"),
            Some("<b>HTML</b>"),
            "<msg@postblox>",
            &attachments,
        ))
        .unwrap();
        assert!(s.contains("multipart/mixed"));
        assert!(s.contains("multipart/alternative"));
        assert!(s.contains("Text"));
        assert!(s.contains("<b>HTML</b>"));
        assert!(s.contains("Content-Disposition: attachment; filename=\"img.png\""));
    }

    #[test]
    fn test_build_mime_with_multiple_attachments() {
        let attachments = vec![
            MimeAttachment {
                filename: "a.txt".into(),
                content_type: "text/plain".into(),
                data: b"aaa".to_vec(),
            },
            MimeAttachment {
                filename: "b.bin".into(),
                content_type: "application/octet-stream".into(),
                data: b"bbb".to_vec(),
            },
        ];
        let s = String::from_utf8(build_mime_with_attachments(
            "bot@example.com",
            &["user@example.com".into()],
            &[],
            "Multi",
            Some("Body"),
            None,
            "<msg@postblox>",
            &attachments,
        ))
        .unwrap();
        assert!(s.contains("filename=\"a.txt\""));
        assert!(s.contains("filename=\"b.bin\""));
        // Count boundary markers: should have opening for body + each attachment + closing
        let mixed_boundary_count = s.matches("postblox-mixed-").count();
        assert!(mixed_boundary_count >= 4); // header + 3 parts + closing
    }

    #[test]
    fn test_build_mime_no_attachments_unchanged() {
        let with = build_mime(
            "bot@example.com",
            &["user@example.com".into()],
            &[],
            "Hello",
            Some("Body text"),
            None,
            "<msg@postblox>",
        );
        let without = build_mime_with_attachments(
            "bot@example.com",
            &["user@example.com".into()],
            &[],
            "Hello",
            Some("Body text"),
            None,
            "<msg@postblox>",
            &[],
        );
        // Both should produce text/plain (no multipart/mixed)
        let s_with = String::from_utf8(with).unwrap();
        let s_without = String::from_utf8(without).unwrap();
        assert!(!s_with.contains("multipart/mixed"));
        assert!(!s_without.contains("multipart/mixed"));
        assert!(s_with.contains("Content-Type: text/plain"));
        assert!(s_without.contains("Content-Type: text/plain"));
    }
}
