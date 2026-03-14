use sqlx::PgPool;
use uuid::Uuid;

use crate::models::TrustScore;

const SELECT_COLS: &str =
    "id, inbox_id, total_sends, approved_count, rejected_count, auto_upgraded, created_at, updated_at";

pub async fn get_or_create(pool: &PgPool, inbox_id: Uuid) -> Result<TrustScore, sqlx::Error> {
    sqlx::query_as(&format!(
        "INSERT INTO trust_scores (inbox_id) VALUES ($1) \
         ON CONFLICT (inbox_id) DO UPDATE SET inbox_id = EXCLUDED.inbox_id \
         RETURNING {SELECT_COLS}"
    ))
    .bind(inbox_id)
    .fetch_one(pool)
    .await
}

pub async fn record_send_outcome(
    pool: &PgPool,
    inbox_id: Uuid,
    approved: bool,
) -> Result<TrustScore, sqlx::Error> {
    sqlx::query_as(&format!(
        "INSERT INTO trust_scores (inbox_id, total_sends, approved_count, rejected_count) \
         VALUES ($1, 1, CASE WHEN $2 THEN 1 ELSE 0 END, CASE WHEN $2 THEN 0 ELSE 1 END) \
         ON CONFLICT (inbox_id) DO UPDATE SET \
         total_sends = trust_scores.total_sends + 1, \
         approved_count = trust_scores.approved_count + CASE WHEN $2 THEN 1 ELSE 0 END, \
         rejected_count = trust_scores.rejected_count + CASE WHEN $2 THEN 0 ELSE 1 END, \
         updated_at = now() \
         RETURNING {SELECT_COLS}"
    ))
    .bind(inbox_id)
    .bind(approved)
    .fetch_one(pool)
    .await
}

pub async fn check_and_upgrade(
    pool: &PgPool,
    inbox_id: Uuid,
    threshold: i32,
) -> Result<Option<TrustScore>, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let result: Option<TrustScore> = sqlx::query_as(&format!(
        "UPDATE trust_scores SET auto_upgraded = true, updated_at = now() \
         WHERE inbox_id = $1 \
         AND approved_count >= $2 \
         AND rejected_count = 0 \
         AND auto_upgraded = false \
         RETURNING {SELECT_COLS}"
    ))
    .bind(inbox_id)
    .bind(threshold)
    .fetch_optional(&mut *tx)
    .await?;

    if result.is_some() {
        sqlx::query(
            "UPDATE permissions SET send_mode = 'auto_approve', updated_at = now() \
             WHERE inbox_id = $1",
        )
        .bind(inbox_id)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_inbox(pool: &PgPool) -> (Uuid, Uuid) {
        let org = crate::db::organizations::create(pool, "Trust Test Org")
            .await
            .unwrap();
        let email = format!("trust-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(pool, org.id, &email, None, "native")
            .await
            .unwrap();
        (org.id, inbox.id)
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_get_or_create_new_trust_score() {
        let pool = crate::db::test_pool().await;
        let (_org_id, inbox_id) = setup_inbox(&pool).await;

        let score = get_or_create(&pool, inbox_id).await.unwrap();
        assert_eq!(score.inbox_id, inbox_id);
        assert_eq!(score.total_sends, 0);
        assert_eq!(score.approved_count, 0);
        assert_eq!(score.rejected_count, 0);
        assert!(!score.auto_upgraded);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_get_or_create_idempotent() {
        let pool = crate::db::test_pool().await;
        let (_org_id, inbox_id) = setup_inbox(&pool).await;

        let s1 = get_or_create(&pool, inbox_id).await.unwrap();
        let s2 = get_or_create(&pool, inbox_id).await.unwrap();
        assert_eq!(s1.id, s2.id);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_record_send_outcome_approved() {
        let pool = crate::db::test_pool().await;
        let (_org_id, inbox_id) = setup_inbox(&pool).await;

        let score = record_send_outcome(&pool, inbox_id, true).await.unwrap();
        assert_eq!(score.total_sends, 1);
        assert_eq!(score.approved_count, 1);
        assert_eq!(score.rejected_count, 0);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_record_send_outcome_rejected() {
        let pool = crate::db::test_pool().await;
        let (_org_id, inbox_id) = setup_inbox(&pool).await;

        let score = record_send_outcome(&pool, inbox_id, false).await.unwrap();
        assert_eq!(score.total_sends, 1);
        assert_eq!(score.approved_count, 0);
        assert_eq!(score.rejected_count, 1);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_record_send_outcome_accumulates() {
        let pool = crate::db::test_pool().await;
        let (_org_id, inbox_id) = setup_inbox(&pool).await;

        record_send_outcome(&pool, inbox_id, true).await.unwrap();
        record_send_outcome(&pool, inbox_id, true).await.unwrap();
        let score = record_send_outcome(&pool, inbox_id, false).await.unwrap();
        assert_eq!(score.total_sends, 3);
        assert_eq!(score.approved_count, 2);
        assert_eq!(score.rejected_count, 1);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_check_and_upgrade_below_threshold_returns_none() {
        let pool = crate::db::test_pool().await;
        let (_org_id, inbox_id) = setup_inbox(&pool).await;

        // 2 approved, threshold is 5
        record_send_outcome(&pool, inbox_id, true).await.unwrap();
        record_send_outcome(&pool, inbox_id, true).await.unwrap();

        let result = check_and_upgrade(&pool, inbox_id, 5).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_check_and_upgrade_at_threshold_upgrades() {
        let pool = crate::db::test_pool().await;
        let (_org_id, inbox_id) = setup_inbox(&pool).await;

        // Create permission row so upgrade has something to modify
        crate::db::permissions::upsert(
            &pool,
            inbox_id,
            crate::models::SendMode::Approval,
            &serde_json::json!({}),
        )
        .await
        .unwrap();

        for _ in 0..3 {
            record_send_outcome(&pool, inbox_id, true).await.unwrap();
        }

        let result = check_and_upgrade(&pool, inbox_id, 3).await.unwrap();
        assert!(result.is_some());
        let score = result.unwrap();
        assert!(score.auto_upgraded);

        // Verify permission was upgraded
        let perm = crate::db::permissions::get_by_inbox(&pool, inbox_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(perm.send_mode, crate::models::SendMode::AutoApprove);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_check_and_upgrade_with_rejections_returns_none() {
        let pool = crate::db::test_pool().await;
        let (_org_id, inbox_id) = setup_inbox(&pool).await;

        for _ in 0..5 {
            record_send_outcome(&pool, inbox_id, true).await.unwrap();
        }
        record_send_outcome(&pool, inbox_id, false).await.unwrap();

        let result = check_and_upgrade(&pool, inbox_id, 5).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_check_and_upgrade_already_upgraded_returns_none() {
        let pool = crate::db::test_pool().await;
        let (_org_id, inbox_id) = setup_inbox(&pool).await;

        crate::db::permissions::upsert(
            &pool,
            inbox_id,
            crate::models::SendMode::Approval,
            &serde_json::json!({}),
        )
        .await
        .unwrap();

        for _ in 0..5 {
            record_send_outcome(&pool, inbox_id, true).await.unwrap();
        }

        let first = check_and_upgrade(&pool, inbox_id, 5).await.unwrap();
        assert!(first.is_some());

        // Second call should return None (already upgraded)
        let second = check_and_upgrade(&pool, inbox_id, 5).await.unwrap();
        assert!(second.is_none());
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_trust_score_cascade_delete_with_inbox() {
        let pool = crate::db::test_pool().await;
        let (_org_id, inbox_id) = setup_inbox(&pool).await;

        get_or_create(&pool, inbox_id).await.unwrap();
        crate::db::inboxes::delete(&pool, inbox_id).await.unwrap();

        // Row should be gone via CASCADE
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT COUNT(*) FROM trust_scores WHERE inbox_id = $1")
                .bind(inbox_id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert_eq!(row.unwrap().0, 0);
    }
}
