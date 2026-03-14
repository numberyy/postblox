use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::auth::{AdminOrg, AuthOrg};
use super::error::ApiError;
use super::{get_inbox_for_org, AppState};
use crate::models::{Permission, SendMode};

#[derive(Debug, Deserialize)]
pub struct UpsertPermissionRequest {
    pub send_mode: Option<SendMode>,
    pub rules: Option<serde_json::Value>,
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(inbox_id): Path<Uuid>,
) -> Result<Json<Permission>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let perm = crate::db::permissions::get_by_inbox(&state.pool, inbox_id)
        .await
        .map_err(ApiError::from_sqlx)?
        .unwrap_or_else(|| Permission::default_for_inbox(inbox_id));

    Ok(Json(perm))
}

pub async fn upsert(
    State(state): State<AppState>,
    AdminOrg(org_id): AdminOrg,
    Path(inbox_id): Path<Uuid>,
    Json(req): Json<UpsertPermissionRequest>,
) -> Result<Json<Permission>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let mode = req.send_mode.unwrap_or_default();

    let rules = match req.rules {
        Some(rules_json) => {
            let parsed = serde_json::from_value::<Vec<crate::core::rules::Rule>>(rules_json)
                .map_err(|e| ApiError::BadRequest(format!("invalid rules: {e}")))?;
            serde_json::to_value(parsed).map_err(|e| ApiError::Internal(e.to_string()))?
        }
        None => serde_json::json!([]),
    };

    let perm = crate::db::permissions::upsert(&state.pool, inbox_id, mode, &rules)
        .await
        .map_err(ApiError::from_sqlx)?;

    let pool = state.pool.clone();
    tokio::spawn(async move {
        crate::events::audit(
            &pool,
            org_id,
            Some(inbox_id),
            crate::models::AuditAction::PermissionChanged,
            "api",
            serde_json::json!({"send_mode": mode.to_string()}),
        )
        .await;
    });

    Ok(Json(perm))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_request_deserialize_minimal() {
        let json = r#"{}"#;
        let req: UpsertPermissionRequest = serde_json::from_str(json).unwrap();
        assert!(req.send_mode.is_none());
        assert!(req.rules.is_none());
    }

    #[test]
    fn test_upsert_request_deserialize_with_mode() {
        let json = r#"{"send_mode": "autonomous"}"#;
        let req: UpsertPermissionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.send_mode, Some(SendMode::Autonomous));
    }

    #[test]
    fn test_upsert_request_deserialize_with_rules() {
        let json = r#"{"send_mode": "auto_approve", "rules": [{"type": "domain_allowlist", "domains": ["ok.com"]}]}"#;
        let req: UpsertPermissionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.send_mode, Some(SendMode::AutoApprove));
        assert!(req.rules.is_some());
        let rules: Vec<crate::core::rules::Rule> =
            serde_json::from_value(req.rules.unwrap()).unwrap();
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn test_upsert_request_deserialize_invalid_send_mode_fails() {
        let json = r#"{"send_mode": "invalid"}"#;
        let result = serde_json::from_str::<UpsertPermissionRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_upsert_request_deserialize_all_modes() {
        for (mode_str, expected) in [
            ("shadow", SendMode::Shadow),
            ("approval", SendMode::Approval),
            ("auto_approve", SendMode::AutoApprove),
            ("autonomous", SendMode::Autonomous),
        ] {
            let json = format!(r#"{{"send_mode": "{mode_str}"}}"#);
            let req: UpsertPermissionRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(req.send_mode, Some(expected));
        }
    }

    #[test]
    fn test_upsert_request_deserialize_rules_empty_array() {
        let json = r#"{"rules": []}"#;
        let req: UpsertPermissionRequest = serde_json::from_str(json).unwrap();
        let rules = req.rules.unwrap();
        assert_eq!(rules, serde_json::json!([]));
    }

    #[test]
    fn test_upsert_request_deserialize_rules_all_types() {
        let json = serde_json::json!({
            "send_mode": "auto_approve",
            "rules": [
                {"type": "domain_allowlist", "domains": ["a.com"]},
                {"type": "domain_blocklist", "domains": ["b.com"]},
                {"type": "time_window", "start_hour": 9, "end_hour": 17, "timezone": "UTC"},
                {"type": "keyword_blocklist", "keywords": ["spam"]},
                {"type": "slop_threshold", "threshold": 0.8}
            ]
        });
        let req: UpsertPermissionRequest = serde_json::from_value(json).unwrap();
        let rules: Vec<crate::core::rules::Rule> =
            serde_json::from_value(req.rules.unwrap()).unwrap();
        assert_eq!(rules.len(), 5);
    }

    #[test]
    fn test_rules_validation_rejects_invalid_type() {
        let invalid_rules = serde_json::json!([{"type": "nonexistent", "foo": "bar"}]);
        let result = serde_json::from_value::<Vec<crate::core::rules::Rule>>(invalid_rules);
        assert!(result.is_err());
    }

    #[test]
    fn test_rules_validation_rejects_missing_fields() {
        let invalid_rules = serde_json::json!([{"type": "domain_allowlist"}]);
        let result = serde_json::from_value::<Vec<crate::core::rules::Rule>>(invalid_rules);
        assert!(result.is_err());
    }
}
