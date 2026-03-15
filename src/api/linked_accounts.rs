use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::AppState;

#[derive(Deserialize)]
pub struct CreateLinkedAccountRequest {
    pub inbox_id: Uuid,
    pub imap_host: String,
    pub imap_port: Option<i32>,
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct SyncResponse {
    pub fetched: usize,
    pub stored: usize,
    pub skipped: usize,
}

pub async fn create(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Json(req): Json<CreateLinkedAccountRequest>,
) -> Result<(StatusCode, Json<crate::models::LinkedAccount>), ApiError> {
    if req.imap_host.is_empty() {
        return Err(ApiError::BadRequest("imap_host is required".into()));
    }
    if req.username.is_empty() {
        return Err(ApiError::BadRequest("username is required".into()));
    }
    if req.password.is_empty() {
        return Err(ApiError::BadRequest("password is required".into()));
    }

    super::get_inbox_for_org(&state.pool, req.inbox_id, org_id).await?;

    let input = crate::models::CreateLinkedAccount {
        inbox_id: req.inbox_id,
        org_id,
        imap_host: req.imap_host,
        imap_port: req.imap_port,
        username: req.username,
        password: req.password,
    };

    let account =
        crate::db::linked_accounts::create(&state.pool, &input, state.encryption_key.as_ref())
            .await
            .map_err(|e| match e {
                crate::db::linked_accounts::LinkedAccountError::Database(db_err) => {
                    ApiError::from_sqlx(db_err)
                }
                other => ApiError::Internal(other.to_string()),
            })?;

    Ok((StatusCode::CREATED, Json(account)))
}

pub async fn list(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
) -> Result<Json<Vec<crate::models::LinkedAccount>>, ApiError> {
    let accounts = crate::db::linked_accounts::list_by_org(&state.pool, org_id)
        .await
        .map_err(ApiError::from_sqlx)?;
    Ok(Json(accounts))
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::models::LinkedAccount>, ApiError> {
    let account = get_account_for_org(&state.pool, id, org_id).await?;
    Ok(Json(account))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    get_account_for_org(&state.pool, id, org_id).await?;
    crate::db::linked_accounts::delete(&state.pool, id)
        .await
        .map_err(ApiError::from_sqlx)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn sync(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(id): Path<Uuid>,
) -> Result<Json<SyncResponse>, ApiError> {
    let account = get_account_for_org(&state.pool, id, org_id).await?;
    let inbox = super::get_inbox_for_org(&state.pool, account.inbox_id, org_id).await?;

    crate::db::linked_accounts::set_sync_status(
        &state.pool,
        account.id,
        crate::sync::SyncStatus::Syncing,
    )
    .await
    .map_err(ApiError::from_sqlx)?;

    let pool_audit = state.pool.clone();
    let account_id = account.id;
    tokio::spawn(async move {
        crate::events::audit(
            &pool_audit,
            org_id,
            Some(inbox.id),
            crate::models::AuditAction::SyncTriggered,
            "api",
            serde_json::json!({"account_id": account_id.to_string()}),
        )
        .await;
    });

    match crate::sync::imap::one_shot_sync(
        &state.pool,
        &account,
        &inbox,
        state.encryption_key.as_ref(),
    )
    .await
    {
        Ok(result) => {
            if let Err(e) = crate::db::linked_accounts::set_sync_status(
                &state.pool,
                account.id,
                crate::sync::SyncStatus::Idle,
            )
            .await
            {
                tracing::warn!(account_id = %account.id, "failed to set sync status to idle: {e}");
            }
            Ok(Json(SyncResponse {
                fetched: result.fetched,
                stored: result.stored,
                skipped: result.skipped,
            }))
        }
        Err(e) => {
            if let Err(status_err) = crate::db::linked_accounts::set_sync_status(
                &state.pool,
                account.id,
                crate::sync::SyncStatus::Error,
            )
            .await
            {
                tracing::warn!(account_id = %account.id, "failed to set sync status to error: {status_err}");
            }
            Err(ApiError::Internal(e.to_string()))
        }
    }
}

async fn get_account_for_org(
    pool: &sqlx::PgPool,
    id: Uuid,
    org_id: Uuid,
) -> Result<crate::models::LinkedAccount, ApiError> {
    let account = crate::db::linked_accounts::get_by_id(pool, id)
        .await
        .map_err(ApiError::from_sqlx)?
        .ok_or(ApiError::NotFound)?;
    if account.org_id != org_id {
        return Err(ApiError::NotFound);
    }
    Ok(account)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_linked_account_request_deserialize() {
        let json = r#"{
            "inbox_id": "00000000-0000-0000-0000-000000000001",
            "imap_host": "imap.gmail.com",
            "imap_port": 993,
            "username": "user@gmail.com",
            "password": "secret"
        }"#;
        let req: CreateLinkedAccountRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.imap_host, "imap.gmail.com");
        assert_eq!(req.imap_port, Some(993));
    }

    #[test]
    fn test_create_linked_account_request_optional_fields() {
        let json = r#"{
            "inbox_id": "00000000-0000-0000-0000-000000000001",
            "imap_host": "imap.example.com",
            "username": "user",
            "password": "pw"
        }"#;
        let req: CreateLinkedAccountRequest = serde_json::from_str(json).unwrap();
        assert!(req.imap_port.is_none());
    }

    #[test]
    fn test_sync_response_serialize() {
        let resp = SyncResponse {
            fetched: 10,
            stored: 8,
            skipped: 2,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["fetched"], 10);
        assert_eq!(json["stored"], 8);
        assert_eq!(json["skipped"], 2);
    }
}
