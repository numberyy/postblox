//! MCP gates and approvals.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::db::DbError;
use crate::models::{ApprovalState, GateAction, McpApproval, McpGate};

// -------- gates --------

const GATE_SELECT: &str = "id, tool, arg_pattern, action, note, created_at";

pub async fn create_gate(
    pool: &SqlitePool,
    tool: &str,
    arg_pattern: Option<&str>,
    action: GateAction,
    note: Option<&str>,
) -> Result<McpGate, DbError> {
    let id = Uuid::new_v4();
    let q = format!(
        "INSERT INTO mcp_gates (id, tool, arg_pattern, action, note) \
         VALUES (?,?,?,?,?) RETURNING {GATE_SELECT}"
    );
    Ok(sqlx::query_as(&q)
        .bind(id)
        .bind(tool)
        .bind(arg_pattern)
        .bind(action)
        .bind(note)
        .fetch_one(pool)
        .await?)
}

pub async fn list_gates(pool: &SqlitePool) -> Result<Vec<McpGate>, DbError> {
    let q = format!("SELECT {GATE_SELECT} FROM mcp_gates ORDER BY tool, created_at");
    Ok(sqlx::query_as(&q).fetch_all(pool).await?)
}

pub async fn list_gates_for_tool(pool: &SqlitePool, tool: &str) -> Result<Vec<McpGate>, DbError> {
    let q = format!("SELECT {GATE_SELECT} FROM mcp_gates WHERE tool = ? ORDER BY created_at");
    Ok(sqlx::query_as(&q).bind(tool).fetch_all(pool).await?)
}

pub async fn delete_gate(pool: &SqlitePool, id: Uuid) -> Result<bool, DbError> {
    let r = sqlx::query("DELETE FROM mcp_gates WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

// -------- approvals --------

const APPROVAL_SELECT: &str = "id, tool, args, summary, state, decided_at, decided_by, created_at";

pub async fn create_approval(
    pool: &SqlitePool,
    tool: &str,
    args: &serde_json::Value,
    summary: &str,
) -> Result<McpApproval, DbError> {
    let id = Uuid::new_v4();
    let q = format!(
        "INSERT INTO mcp_approvals (id, tool, args, summary) \
         VALUES (?,?,?,?) RETURNING {APPROVAL_SELECT}"
    );
    Ok(sqlx::query_as(&q)
        .bind(id)
        .bind(tool)
        .bind(args)
        .bind(summary)
        .fetch_one(pool)
        .await?)
}

pub async fn get_approval(pool: &SqlitePool, id: Uuid) -> Result<Option<McpApproval>, DbError> {
    let q = format!("SELECT {APPROVAL_SELECT} FROM mcp_approvals WHERE id = ?");
    Ok(sqlx::query_as(&q).bind(id).fetch_optional(pool).await?)
}

pub async fn list_approvals(
    pool: &SqlitePool,
    state: Option<ApprovalState>,
    limit: i64,
    offset: i64,
) -> Result<Vec<McpApproval>, DbError> {
    let limit = limit.clamp(1, 500);
    let offset = offset.max(0);
    // Tie-break on rowid so pagination is deterministic when timestamps
    // collide on the same millisecond.
    let rows = match state {
        Some(s) => {
            let q = format!(
                "SELECT {APPROVAL_SELECT} FROM mcp_approvals WHERE state = ? \
                 ORDER BY created_at DESC, rowid DESC LIMIT ? OFFSET ?"
            );
            sqlx::query_as(&q)
                .bind(s)
                .bind(limit)
                .bind(offset)
                .fetch_all(pool)
                .await?
        }
        None => {
            let q = format!(
                "SELECT {APPROVAL_SELECT} FROM mcp_approvals \
                 ORDER BY created_at DESC, rowid DESC LIMIT ? OFFSET ?"
            );
            sqlx::query_as(&q)
                .bind(limit)
                .bind(offset)
                .fetch_all(pool)
                .await?
        }
    };
    Ok(rows)
}

/// Move an approval to a terminal state. Returns `Ok(true)` only if the
/// row was still pending — prevents double-decision races.
pub async fn decide(
    pool: &SqlitePool,
    id: Uuid,
    new_state: ApprovalState,
    decided_by: &str,
) -> Result<bool, DbError> {
    debug_assert!(matches!(
        new_state,
        ApprovalState::Allowed | ApprovalState::Denied | ApprovalState::Expired
    ));
    let now: DateTime<Utc> = Utc::now();
    let r = sqlx::query(
        "UPDATE mcp_approvals SET state = ?, decided_at = ?, decided_by = ? \
         WHERE id = ? AND state = 'pending'",
    )
    .bind(new_state)
    .bind(now)
    .bind(decided_by)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_gate_round_trip() {
        let pool = crate::db::test_pool().await;
        let g = create_gate(
            &pool,
            "send",
            Some(r#"{"to":"*@x.com"}"#),
            GateAction::AutoAllow,
            Some("trusted recipients"),
        )
        .await
        .unwrap();
        assert_eq!(g.tool, "send");
        assert_eq!(g.action, GateAction::AutoAllow);

        let listed = list_gates(&pool).await.unwrap();
        assert_eq!(listed.len(), 1);
        let by_tool = list_gates_for_tool(&pool, "send").await.unwrap();
        assert_eq!(by_tool[0].id, g.id);
        assert!(list_gates_for_tool(&pool, "delete")
            .await
            .unwrap()
            .is_empty());

        assert!(delete_gate(&pool, g.id).await.unwrap());
        assert!(!delete_gate(&pool, g.id).await.unwrap());
    }

    #[tokio::test]
    async fn test_approval_starts_pending() {
        let pool = crate::db::test_pool().await;
        let a = create_approval(&pool, "send", &json!({"to": ["x@y"]}), "send to x@y")
            .await
            .unwrap();
        assert_eq!(a.state, ApprovalState::Pending);
        assert!(a.decided_at.is_none());
    }

    #[tokio::test]
    async fn test_decide_allows_once_only() {
        let pool = crate::db::test_pool().await;
        let a = create_approval(&pool, "delete", &json!({}), "delete one")
            .await
            .unwrap();

        assert!(decide(&pool, a.id, ApprovalState::Allowed, "user")
            .await
            .unwrap());
        // second decision must be a no-op (returns false)
        assert!(!decide(&pool, a.id, ApprovalState::Denied, "user")
            .await
            .unwrap());

        let got = get_approval(&pool, a.id).await.unwrap().unwrap();
        assert_eq!(got.state, ApprovalState::Allowed);
        assert_eq!(got.decided_by.as_deref(), Some("user"));
        assert!(got.decided_at.is_some());
    }

    #[tokio::test]
    async fn test_list_filters_by_state() {
        let pool = crate::db::test_pool().await;
        let a = create_approval(&pool, "send", &json!({}), "x")
            .await
            .unwrap();
        let b = create_approval(&pool, "send", &json!({}), "y")
            .await
            .unwrap();
        decide(&pool, a.id, ApprovalState::Allowed, "user")
            .await
            .unwrap();

        let pending = list_approvals(&pool, Some(ApprovalState::Pending), 50, 0)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, b.id);

        let allowed = list_approvals(&pool, Some(ApprovalState::Allowed), 50, 0)
            .await
            .unwrap();
        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed[0].id, a.id);

        let all = list_approvals(&pool, None, 50, 0).await.unwrap();
        assert_eq!(all.len(), 2);
    }
}
