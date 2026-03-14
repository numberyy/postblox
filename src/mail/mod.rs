pub mod builder;
pub mod error;
pub mod guard;
pub mod parser;
pub mod reply_extract;
pub mod threading;

pub use threading::{ThreadMatch, ThreadRef};

use uuid::Uuid;

pub fn parsed_to_create_message(
    parsed: &parser::ParsedEmail,
    inbox_id: Uuid,
    thread_id: Option<Uuid>,
    extracted_text: Option<String>,
) -> crate::models::CreateMessage {
    crate::models::CreateMessage {
        inbox_id,
        thread_id,
        message_id_header: parsed.message_id.clone(),
        in_reply_to: parsed.in_reply_to.clone(),
        references_header: if parsed.references.is_empty() {
            None
        } else {
            Some(parsed.references.join(" "))
        },
        from_addr: parsed.from.clone(),
        to_addrs: serde_json::json!(parsed.to),
        cc_addrs: if parsed.cc.is_empty() {
            None
        } else {
            Some(serde_json::json!(parsed.cc))
        },
        subject: parsed.subject.clone(),
        text_body: parsed.text_body.clone(),
        html_body: parsed.html_body.clone(),
        extracted_text,
        direction: crate::models::Direction::Inbound,
        raw_headers: Some(parsed.raw_headers.clone()),
    }
}
