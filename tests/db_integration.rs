//! End-to-end smoke test for the SQLite layer.
//!
//! Runs against a real on-disk DB so the migration file path, WAL mode,
//! and foreign-key cascades are exercised together.

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use postblox::db::connect;
use postblox::db::{accounts, attachments, audit, drafts, folders, mcp, messages, search, threads};
use postblox::models::{ApprovalState, AttachmentDisposition, AuthKind, FolderRole, GateAction};

#[tokio::test]
async fn end_to_end_account_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("postblox.db");
    let pool = connect(&path).await.unwrap();

    // Create account.
    let acc = accounts::create(
        &pool,
        &accounts::NewAccount {
            email: "alice@example.com".into(),
            display_name: Some("Alice".into()),
            auth_kind: AuthKind::Password,
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            imap_use_tls: true,
            smtp_host: "smtp.example.com".into(),
            smtp_port: 465,
            smtp_use_tls: true,
            smtp_starttls: false,
        },
    )
    .await
    .unwrap();

    // Folders.
    let inbox = folders::upsert(
        &pool,
        &folders::NewFolder {
            account_id: acc.id,
            name: "INBOX".into(),
            delimiter: "/".into(),
            role: FolderRole::Inbox,
            selectable: true,
        },
    )
    .await
    .unwrap();

    // Thread + message.
    let thread = threads::create(&pool, acc.id, Some("gmail-1"), Some("Hi"))
        .await
        .unwrap();
    let msg = messages::create(
        &pool,
        &messages::NewMessage {
            account_id: acc.id,
            folder_id: inbox.id,
            thread_id: Some(thread.id),
            uid: 1,
            message_id_header: Some("<m1@example.com>".into()),
            in_reply_to: None,
            references_header: None,
            from_addr: "bob@example.com".into(),
            to_addrs: json!(["alice@example.com"]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            reply_to: None,
            subject: Some("Quarterly invoice".into()),
            snippet: Some("please review".into()),
            text_body: Some("please review attached invoice".into()),
            html_body: None,
            raw_size: 4096,
            flags: json!([]),
            internal_date: Utc::now(),
            sent_at: None,
        },
    )
    .await
    .unwrap();

    // Attachment.
    attachments::create(
        &pool,
        &attachments::NewAttachment {
            message_id: msg.id,
            filename: "invoice.pdf".into(),
            content_type: "application/pdf".into(),
            content_id: None,
            size_bytes: 2048,
            disposition: AttachmentDisposition::Attachment,
            storage_path: "abc/invoice.pdf".into(),
        },
    )
    .await
    .unwrap();

    // Draft.
    drafts::create(
        &pool,
        &drafts::NewDraft {
            account_id: acc.id,
            in_reply_to_msg: Some(msg.id),
            to_addrs: json!(["bob@example.com"]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            subject: Some("Re: Quarterly invoice".into()),
            text_body: Some("got it, thanks".into()),
            html_body: None,
            in_reply_to: None,
            references_header: None,
        },
    )
    .await
    .unwrap();

    // FTS search.
    let hits = search::search(&pool, &search::quote_term("invoice"), 50, 0)
        .await
        .unwrap();
    assert_eq!(
        hits.len(),
        1,
        "expect FTS hit on subject 'Quarterly invoice'"
    );

    // MCP gate + approval.
    mcp::create_gate(
        &pool,
        "send",
        Some(r#"{"to":"*@example.com"}"#),
        GateAction::AutoAllow,
        None,
    )
    .await
    .unwrap();
    let approval = mcp::create_approval(
        &pool,
        "delete",
        &json!({"message_id": msg.id.to_string()}),
        "delete invoice message",
    )
    .await
    .unwrap();
    assert_eq!(approval.state, ApprovalState::Pending);

    // Audit entry.
    audit::record(
        &pool,
        &audit::NewAuditEntry {
            actor: "user".into(),
            action: "open_thread".into(),
            target: Some(thread.id.to_string()),
            details: json!({}),
        },
    )
    .await
    .unwrap();

    // Sanity: lists return what we expect.
    assert_eq!(
        folders::list_by_account(&pool, acc.id).await.unwrap().len(),
        1
    );
    assert_eq!(
        messages::list_by_folder(&pool, inbox.id, 50, 0)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        messages::list_by_thread(&pool, thread.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        attachments::list_for_message(&pool, msg.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        drafts::list_by_account(&pool, acc.id).await.unwrap().len(),
        1
    );
    assert_eq!(audit::list_recent(&pool, 50, 0).await.unwrap().len(), 1);

    // Deleting the account cascades through folder → messages → attachments.
    accounts::delete(&pool, acc.id).await.unwrap();
    assert!(folders::list_by_account(&pool, acc.id)
        .await
        .unwrap()
        .is_empty());
    assert!(messages::get(&pool, msg.id).await.unwrap().is_none());
    assert!(attachments::list_for_message(&pool, msg.id)
        .await
        .unwrap()
        .is_empty());

    // Audit is decoupled from accounts; it survives.
    assert_eq!(audit::list_recent(&pool, 50, 0).await.unwrap().len(), 1);

    // Pool is reusable after cascade.
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0);

    // Hide an unused-import warning when fields are added later.
    let _ = Uuid::new_v4();
}

#[tokio::test]
async fn connect_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("postblox.db");

    // First connect creates the schema + an account.
    {
        let pool = connect(&path).await.unwrap();
        accounts::create(
            &pool,
            &accounts::NewAccount {
                email: "p@x.com".into(),
                display_name: None,
                auth_kind: AuthKind::Password,
                imap_host: "i".into(),
                imap_port: 993,
                imap_use_tls: true,
                smtp_host: "s".into(),
                smtp_port: 465,
                smtp_use_tls: true,
                smtp_starttls: false,
            },
        )
        .await
        .unwrap();
    }

    // Reconnect, verify the row survived and migrations are a no-op.
    let pool = connect(&path).await.unwrap();
    let listed = accounts::list(&pool).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].email, "p@x.com");
}
