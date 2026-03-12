use chrono::Utc;
use uuid::Uuid;

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
    let mut msg = String::new();
    msg.push_str(&format!("From: {}\r\n", sanitize_header(from)));
    msg.push_str(&format!("To: {}\r\n", sanitize_header(&to.join(", "))));
    if !cc.is_empty() {
        msg.push_str(&format!("Cc: {}\r\n", sanitize_header(&cc.join(", "))));
    }
    msg.push_str(&format!("Subject: {}\r\n", sanitize_header(subject)));
    msg.push_str(&format!("Message-ID: {}\r\n", sanitize_header(message_id)));
    msg.push_str(&format!(
        "Date: {}\r\n",
        Utc::now().format("%a, %d %b %Y %H:%M:%S +0000")
    ));
    msg.push_str("MIME-Version: 1.0\r\n");

    match (text_body, html_body) {
        (Some(text), Some(html)) => {
            let boundary = format!("postblox-{}", Uuid::new_v4().simple());
            msg.push_str(&format!(
                "Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n"
            ));
            msg.push_str("\r\n");
            msg.push_str(&format!("--{boundary}\r\n"));
            msg.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
            msg.push_str(text);
            msg.push_str(&format!("\r\n--{boundary}\r\n"));
            msg.push_str("Content-Type: text/html; charset=utf-8\r\n\r\n");
            msg.push_str(html);
            msg.push_str(&format!("\r\n--{boundary}--\r\n"));
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

    msg.into_bytes()
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
}
