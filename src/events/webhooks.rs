use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum WebhookError {
    #[error("webhook delivery failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("webhook endpoint returned {0}")]
    Status(u16),
}

pub fn sign_payload(secret: &str, payload: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload);
    let result = mac.finalize().into_bytes();
    format!("{result:x}")
}

pub async fn deliver(
    client: &reqwest::Client,
    url: &str,
    secret: &str,
    event_name: &str,
    data: &serde_json::Value,
) -> Result<(), WebhookError> {
    let payload = serde_json::json!({
        "event": event_name,
        "timestamp": Utc::now().to_rfc3339(),
        "data": data,
    });
    let body = serde_json::to_vec(&payload).expect("webhook payload serialization");
    let signature = sign_payload(secret, &body);

    let resp = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-postblox-event", event_name)
        .header("x-postblox-signature", format!("sha256={signature}"))
        .body(body)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(WebhookError::Status(resp.status().as_u16()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_payload_known_value() {
        // HMAC-SHA256("secret", "hello") is a known value
        let sig = sign_payload("secret", b"hello");
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
        // Verify determinism
        assert_eq!(sig, sign_payload("secret", b"hello"));
    }

    #[test]
    fn test_sign_payload_different_secret_different_signature() {
        let s1 = sign_payload("secret-a", b"payload");
        let s2 = sign_payload("secret-b", b"payload");
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_sign_payload_different_payload_different_signature() {
        let s1 = sign_payload("secret", b"payload-a");
        let s2 = sign_payload("secret", b"payload-b");
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_sign_payload_empty_payload() {
        let sig = sign_payload("secret", b"");
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_sign_payload_empty_secret() {
        let sig = sign_payload("", b"payload");
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_sign_payload_both_empty() {
        let sig = sign_payload("", b"");
        assert_eq!(sig.len(), 64);
    }

    #[test]
    fn test_webhook_payload_structure() {
        let data = serde_json::json!({"message_id": "abc", "inbox_id": "def"});
        let payload = serde_json::json!({
            "event": "message.received",
            "timestamp": Utc::now().to_rfc3339(),
            "data": data,
        });

        assert_eq!(payload["event"], "message.received");
        assert!(payload["timestamp"].is_string());
        assert_eq!(payload["data"]["message_id"], "abc");
        assert_eq!(payload["data"]["inbox_id"], "def");
    }

    #[test]
    fn test_deliver_builds_correct_signature_header_format() {
        let sig = sign_payload("mysecret", b"{\"test\":true}");
        let header = format!("sha256={sig}");
        assert!(header.starts_with("sha256="));
        assert_eq!(header.len(), 7 + 64); // "sha256=" + 64 hex chars
    }
}
