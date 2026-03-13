use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Approval, ApprovalStatus, ApprovalWithDetails, CreateApproval};

const SELECT_COLS: &str =
    "id, org_id, inbox_id, message_id, status, decided_by, decided_at, created_at";

pub async fn create(pool: &PgPool, input: &CreateApproval) -> Result<Approval, sqlx::Error> {
    let query = format!(
        "INSERT INTO approvals (org_id, inbox_id, message_id) \
         VALUES ($1, $2, $3) \
         RETURNING {SELECT_COLS}"
    );
    sqlx::query_as(&query)
        .bind(input.org_id)
        .bind(input.inbox_id)
        .bind(input.message_id)
        .fetch_one(pool)
        .await
}

pub async fn list_by_status(
    pool: &PgPool,
    org_id: Uuid,
    status: Option<&str>,
    offset: i64,
    limit: i64,
) -> Result<Vec<Approval>, sqlx::Error> {
    match status {
        Some(s) => {
            let query = format!(
                "SELECT {SELECT_COLS} FROM approvals \
                 WHERE org_id = $1 AND status = $2 \
                 ORDER BY created_at ASC LIMIT $3 OFFSET $4"
            );
            sqlx::query_as(&query)
                .bind(org_id)
                .bind(s)
                .bind(limit)
                .bind(offset)
                .fetch_all(pool)
                .await
        }
        None => {
            let query = format!(
                "SELECT {SELECT_COLS} FROM approvals \
                 WHERE org_id = $1 \
                 ORDER BY created_at ASC LIMIT $2 OFFSET $3"
            );
            sqlx::query_as(&query)
                .bind(org_id)
                .bind(limit)
                .bind(offset)
                .fetch_all(pool)
                .await
        }
    }
}

pub async fn count_by_status(
    pool: &PgPool,
    org_id: Uuid,
    status: ApprovalStatus,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM approvals WHERE org_id = $1 AND status = $2")
            .bind(org_id)
            .bind(status.to_string())
            .fetch_one(pool)
            .await?;
    Ok(row.0)
}

/// Returns pending approvals with joined message subject/from and inbox email.
pub async fn list_pending_with_details(
    pool: &PgPool,
    org_id: Uuid,
    limit: i64,
) -> Result<Vec<ApprovalWithDetails>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.created_at, m.subject, m.from_addr, i.email AS inbox_email \
         FROM approvals a \
         JOIN messages m ON m.id = a.message_id \
         JOIN inboxes i ON i.id = a.inbox_id \
         WHERE a.org_id = $1 AND a.status = 'pending' \
         ORDER BY a.created_at ASC LIMIT $2",
    )
    .bind(org_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn get(
    pool: &PgPool,
    org_id: Uuid,
    approval_id: Uuid,
) -> Result<Option<Approval>, sqlx::Error> {
    let query = format!("SELECT {SELECT_COLS} FROM approvals WHERE id = $1 AND org_id = $2");
    sqlx::query_as(&query)
        .bind(approval_id)
        .bind(org_id)
        .fetch_optional(pool)
        .await
}

pub async fn approve(
    pool: &PgPool,
    org_id: Uuid,
    approval_id: Uuid,
    decided_by: &str,
) -> Result<Option<Approval>, sqlx::Error> {
    let query = format!(
        "UPDATE approvals SET status = 'approved', decided_by = $3, decided_at = now() \
         WHERE id = $1 AND org_id = $2 AND status = 'pending' \
         RETURNING {SELECT_COLS}"
    );
    sqlx::query_as(&query)
        .bind(approval_id)
        .bind(org_id)
        .bind(decided_by)
        .fetch_optional(pool)
        .await
}

pub async fn reject(
    pool: &PgPool,
    org_id: Uuid,
    approval_id: Uuid,
    decided_by: &str,
) -> Result<Option<Approval>, sqlx::Error> {
    let query = format!(
        "UPDATE approvals SET status = 'rejected', decided_by = $3, decided_at = now() \
         WHERE id = $1 AND org_id = $2 AND status = 'pending' \
         RETURNING {SELECT_COLS}"
    );
    sqlx::query_as(&query)
        .bind(approval_id)
        .bind(org_id)
        .bind(decided_by)
        .fetch_optional(pool)
        .await
}

pub async fn batch_decide(
    pool: &PgPool,
    org_id: Uuid,
    approval_ids: &[Uuid],
    status: ApprovalStatus,
    decided_by: &str,
) -> Result<Vec<Approval>, sqlx::Error> {
    if approval_ids.is_empty() {
        return Ok(vec![]);
    }
    let query = format!(
        "UPDATE approvals SET status = $3, decided_by = $4, decided_at = now() \
         WHERE org_id = $1 AND id = ANY($2) AND status = 'pending' \
         RETURNING {SELECT_COLS}"
    );
    sqlx::query_as(&query)
        .bind(org_id)
        .bind(approval_ids)
        .bind(status.to_string())
        .bind(decided_by)
        .fetch_all(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_approval(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
        let org = crate::db::organizations::create(pool, "Approval Test Org")
            .await
            .unwrap();
        let email = format!("appr-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(pool, org.id, &email, None, "native")
            .await
            .unwrap();
        let cm = crate::models::CreateMessage {
            inbox_id: inbox.id,
            thread_id: None,
            message_id_header: Some(format!("<{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: "sender@example.com".into(),
            to_addrs: serde_json::json!(["rcpt@example.com"]),
            cc_addrs: None,
            subject: Some("Test".into()),
            text_body: Some("Hello".into()),
            html_body: None,
            extracted_text: None,
            direction: "outbound".into(),
            raw_headers: None,
        };
        let msg = crate::db::messages::create(pool, &cm).await.unwrap();
        (org.id, inbox.id, msg.id)
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_create_approval() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, message_id) = setup_approval(&pool).await;

        let input = CreateApproval {
            org_id,
            inbox_id,
            message_id,
        };
        let approval = create(&pool, &input).await.unwrap();
        assert_eq!(approval.org_id, org_id);
        assert_eq!(approval.inbox_id, inbox_id);
        assert_eq!(approval.message_id, message_id);
        assert_eq!(approval.status, "pending");
        assert!(approval.decided_by.is_none());
        assert!(approval.decided_at.is_none());
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_list_pending_only() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, message_id) = setup_approval(&pool).await;

        let input = CreateApproval {
            org_id,
            inbox_id,
            message_id,
        };
        let a1 = create(&pool, &input).await.unwrap();

        // Create another message for second approval
        let cm2 = crate::models::CreateMessage {
            inbox_id,
            thread_id: None,
            message_id_header: Some(format!("<{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: "sender@example.com".into(),
            to_addrs: serde_json::json!(["rcpt@example.com"]),
            cc_addrs: None,
            subject: Some("Test 2".into()),
            text_body: Some("Hello 2".into()),
            html_body: None,
            extracted_text: None,
            direction: "outbound".into(),
            raw_headers: None,
        };
        let msg2 = crate::db::messages::create(&pool, &cm2).await.unwrap();
        let input2 = CreateApproval {
            org_id,
            inbox_id,
            message_id: msg2.id,
        };
        create(&pool, &input2).await.unwrap();

        // Approve the first one
        approve(&pool, org_id, a1.id, "admin").await.unwrap();

        let pending = list_by_status(&pool, org_id, Some("pending"), 0, 100)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].message_id, msg2.id);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_get_approval() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, message_id) = setup_approval(&pool).await;

        let input = CreateApproval {
            org_id,
            inbox_id,
            message_id,
        };
        let a = create(&pool, &input).await.unwrap();

        let fetched = get(&pool, org_id, a.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, a.id);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_get_approval_wrong_org_returns_none() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, message_id) = setup_approval(&pool).await;

        let input = CreateApproval {
            org_id,
            inbox_id,
            message_id,
        };
        let a = create(&pool, &input).await.unwrap();

        let other_org = crate::db::organizations::create(&pool, "Other Approval Org")
            .await
            .unwrap();
        let result = get(&pool, other_org.id, a.id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_approve_sets_status_and_decided_by() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, message_id) = setup_approval(&pool).await;

        let input = CreateApproval {
            org_id,
            inbox_id,
            message_id,
        };
        let a = create(&pool, &input).await.unwrap();

        let approved = approve(&pool, org_id, a.id, "admin@example.com")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(approved.status, "approved");
        assert_eq!(approved.decided_by.as_deref(), Some("admin@example.com"));
        assert!(approved.decided_at.is_some());
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_approve_already_approved_returns_none() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, message_id) = setup_approval(&pool).await;

        let input = CreateApproval {
            org_id,
            inbox_id,
            message_id,
        };
        let a = create(&pool, &input).await.unwrap();

        approve(&pool, org_id, a.id, "admin").await.unwrap();
        let second = approve(&pool, org_id, a.id, "admin2").await.unwrap();
        assert!(second.is_none());
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_reject_sets_status() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, message_id) = setup_approval(&pool).await;

        let input = CreateApproval {
            org_id,
            inbox_id,
            message_id,
        };
        let a = create(&pool, &input).await.unwrap();

        let rejected = reject(&pool, org_id, a.id, "moderator")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rejected.status, "rejected");
        assert_eq!(rejected.decided_by.as_deref(), Some("moderator"));
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_reject_already_rejected_returns_none() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, message_id) = setup_approval(&pool).await;

        let input = CreateApproval {
            org_id,
            inbox_id,
            message_id,
        };
        let a = create(&pool, &input).await.unwrap();

        reject(&pool, org_id, a.id, "mod").await.unwrap();
        let second = reject(&pool, org_id, a.id, "mod2").await.unwrap();
        assert!(second.is_none());
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_batch_decide_approves_multiple() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, message_id) = setup_approval(&pool).await;

        let a1 = create(
            &pool,
            &CreateApproval {
                org_id,
                inbox_id,
                message_id,
            },
        )
        .await
        .unwrap();

        let cm2 = crate::models::CreateMessage {
            inbox_id,
            thread_id: None,
            message_id_header: Some(format!("<{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: "sender@example.com".into(),
            to_addrs: serde_json::json!(["rcpt@example.com"]),
            cc_addrs: None,
            subject: Some("Batch".into()),
            text_body: Some("Batch body".into()),
            html_body: None,
            extracted_text: None,
            direction: "outbound".into(),
            raw_headers: None,
        };
        let msg2 = crate::db::messages::create(&pool, &cm2).await.unwrap();
        let a2 = create(
            &pool,
            &CreateApproval {
                org_id,
                inbox_id,
                message_id: msg2.id,
            },
        )
        .await
        .unwrap();

        let decided = batch_decide(
            &pool,
            org_id,
            &[a1.id, a2.id],
            ApprovalStatus::Approved,
            "batch_admin",
        )
        .await
        .unwrap();
        assert_eq!(decided.len(), 2);
        for d in &decided {
            assert_eq!(d.status, "approved");
            assert_eq!(d.decided_by.as_deref(), Some("batch_admin"));
        }
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_batch_decide_empty_ids_returns_empty() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Batch Empty Org")
            .await
            .unwrap();
        let result = batch_decide(&pool, org.id, &[], ApprovalStatus::Approved, "admin")
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_batch_decide_skips_already_decided() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, message_id) = setup_approval(&pool).await;

        let a1 = create(
            &pool,
            &CreateApproval {
                org_id,
                inbox_id,
                message_id,
            },
        )
        .await
        .unwrap();

        // Pre-approve a1
        approve(&pool, org_id, a1.id, "admin").await.unwrap();

        let cm2 = crate::models::CreateMessage {
            inbox_id,
            thread_id: None,
            message_id_header: Some(format!("<{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: "sender@example.com".into(),
            to_addrs: serde_json::json!(["rcpt@example.com"]),
            cc_addrs: None,
            subject: Some("Batch skip".into()),
            text_body: Some("Body".into()),
            html_body: None,
            extracted_text: None,
            direction: "outbound".into(),
            raw_headers: None,
        };
        let msg2 = crate::db::messages::create(&pool, &cm2).await.unwrap();
        let a2 = create(
            &pool,
            &CreateApproval {
                org_id,
                inbox_id,
                message_id: msg2.id,
            },
        )
        .await
        .unwrap();

        let decided = batch_decide(
            &pool,
            org_id,
            &[a1.id, a2.id],
            ApprovalStatus::Rejected,
            "mod",
        )
        .await
        .unwrap();
        // Only a2 should be decided (a1 was already approved)
        assert_eq!(decided.len(), 1);
        assert_eq!(decided[0].id, a2.id);
        assert_eq!(decided[0].status, "rejected");
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_approve_nonexistent_returns_none() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Ghost Approval Org")
            .await
            .unwrap();
        let result = approve(&pool, org.id, Uuid::new_v4(), "admin")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_approval_cascade_deletes_on_message_delete() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, message_id) = setup_approval(&pool).await;

        let input = CreateApproval {
            org_id,
            inbox_id,
            message_id,
        };
        let approval = create(&pool, &input).await.unwrap();

        // Delete the message — ON DELETE CASCADE should remove the approval
        sqlx::query("DELETE FROM messages WHERE id = $1")
            .bind(message_id)
            .execute(&pool)
            .await
            .unwrap();

        let fetched = get(&pool, org_id, approval.id).await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_list_all_statuses() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, message_id) = setup_approval(&pool).await;

        let a1 = create(
            &pool,
            &CreateApproval {
                org_id,
                inbox_id,
                message_id,
            },
        )
        .await
        .unwrap();

        let cm2 = crate::models::CreateMessage {
            inbox_id,
            thread_id: None,
            message_id_header: Some(format!("<{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: "sender@example.com".into(),
            to_addrs: serde_json::json!(["rcpt@example.com"]),
            cc_addrs: None,
            subject: Some("All status".into()),
            text_body: Some("Body".into()),
            html_body: None,
            extracted_text: None,
            direction: "outbound".into(),
            raw_headers: None,
        };
        let msg2 = crate::db::messages::create(&pool, &cm2).await.unwrap();
        create(
            &pool,
            &CreateApproval {
                org_id,
                inbox_id,
                message_id: msg2.id,
            },
        )
        .await
        .unwrap();

        // Approve a1
        approve(&pool, org_id, a1.id, "admin").await.unwrap();

        // list_by_status with None returns all (both pending and approved)
        let all = list_by_status(&pool, org_id, None, 0, 100).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_list_pending_pagination() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id, _) = setup_approval(&pool).await;

        for _ in 0..5 {
            let cm = crate::models::CreateMessage {
                inbox_id,
                thread_id: None,
                message_id_header: Some(format!("<{}>", Uuid::new_v4())),
                in_reply_to: None,
                references_header: None,
                from_addr: "sender@example.com".into(),
                to_addrs: serde_json::json!(["rcpt@example.com"]),
                cc_addrs: None,
                subject: Some("Page test".into()),
                text_body: Some("Body".into()),
                html_body: None,
                extracted_text: None,
                direction: "outbound".into(),
                raw_headers: None,
            };
            let msg = crate::db::messages::create(&pool, &cm).await.unwrap();
            create(
                &pool,
                &CreateApproval {
                    org_id,
                    inbox_id,
                    message_id: msg.id,
                },
            )
            .await
            .unwrap();
        }

        let page1 = list_by_status(&pool, org_id, Some("pending"), 0, 3)
            .await
            .unwrap();
        assert_eq!(page1.len(), 3);

        let page2 = list_by_status(&pool, org_id, Some("pending"), 3, 3)
            .await
            .unwrap();
        assert_eq!(page2.len(), 2);
    }
}
