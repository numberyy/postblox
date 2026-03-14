use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::{AdminOrg, AuthOrg};
use super::error::ApiError;
use super::AppState;
use crate::models::Domain;

#[derive(Deserialize)]
pub struct CreateDomainRequest {
    pub name: String,
}

#[derive(Serialize)]
pub struct DomainWithDns {
    #[serde(flatten)]
    pub domain: Domain,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_records: Option<serde_json::Value>,
}

fn is_valid_domain(name: &str) -> bool {
    if name.is_empty() || name.len() > 253 {
        return false;
    }
    if !name.contains('.')
        || name.starts_with('.')
        || name.ends_with('.')
        || name.contains("..")
        || name.starts_with("http://")
        || name.starts_with("https://")
    {
        return false;
    }
    name.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

pub async fn create(
    State(state): State<AppState>,
    AdminOrg(org_id): AdminOrg,
    Json(req): Json<CreateDomainRequest>,
) -> Result<(StatusCode, Json<Domain>), ApiError> {
    let name = req.name.trim().to_lowercase();
    if !is_valid_domain(&name) {
        return Err(ApiError::BadRequest("invalid domain name".into()));
    }

    let mut domain = crate::db::domains::create(&state.pool, org_id, &name)
        .await
        .map_err(ApiError::from_sqlx)?;

    if let Some(ref stalwart) = state.stalwart {
        match stalwart.create_domain(&name).await {
            Ok(principal_id) => {
                match crate::db::domains::update_status(
                    &state.pool,
                    domain.id,
                    "pending",
                    Some(&principal_id),
                )
                .await
                {
                    Ok(Some(updated)) => domain = updated,
                    Ok(None) => tracing::warn!("domain {name} disappeared during status update"),
                    Err(e) => tracing::warn!("failed to update domain status for {name}: {e}"),
                }
            }
            Err(e) => {
                tracing::error!("stalwart domain creation failed for {name}: {e}");
                if let Err(re) = crate::db::domains::delete(&state.pool, domain.id).await {
                    tracing::error!("rollback delete of domain {} also failed: {re}", domain.id);
                }
                return Err(ApiError::Internal(format!(
                    "mail server domain creation failed: {e}"
                )));
            }
        }
    }

    Ok((StatusCode::CREATED, Json(domain)))
}

pub async fn list(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
) -> Result<Json<Vec<Domain>>, ApiError> {
    let domains = crate::db::domains::list_by_org(&state.pool, org_id)
        .await
        .map_err(ApiError::from_sqlx)?;
    Ok(Json(domains))
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(id): Path<Uuid>,
) -> Result<Json<DomainWithDns>, ApiError> {
    let domain = get_domain_for_org(&state.pool, id, org_id).await?;

    let dns_records = if let Some(ref stalwart) = state.stalwart {
        if domain.stalwart_principal_id.is_some() {
            match stalwart.get_dns_records(&domain.name).await {
                Ok(r) => Some(r),
                Err(e) => {
                    tracing::error!(domain = %domain.name, "failed to fetch DNS records: {e}");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    Ok(Json(DomainWithDns {
        domain,
        dns_records,
    }))
}

pub async fn verify(
    State(state): State<AppState>,
    AdminOrg(org_id): AdminOrg,
    Path(id): Path<Uuid>,
) -> Result<Json<Domain>, ApiError> {
    let domain = get_domain_for_org(&state.pool, id, org_id).await?;

    let stalwart = state
        .stalwart
        .as_ref()
        .ok_or_else(|| ApiError::Internal("stalwart not configured".into()))?;

    let dns = stalwart
        .get_dns_records(&domain.name)
        .await
        .map_err(|e| ApiError::Internal(format!("DNS lookup failed: {e}")))?;

    let records = dns["data"].as_array();
    let verified = records.is_some_and(|r| !r.is_empty());

    let updated = if verified {
        crate::db::domains::set_verified(&state.pool, domain.id).await
    } else {
        crate::db::domains::update_status(&state.pool, domain.id, "failed", None).await
    }
    .map_err(ApiError::from_sqlx)?
    .ok_or(ApiError::NotFound)?;

    Ok(Json(updated))
}

pub async fn delete(
    State(state): State<AppState>,
    AdminOrg(org_id): AdminOrg,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let domain = get_domain_for_org(&state.pool, id, org_id).await?;

    if let Some(ref stalwart) = state.stalwart {
        if let Some(ref principal_id) = domain.stalwart_principal_id {
            if let Err(e) = stalwart.delete_domain(principal_id).await {
                tracing::warn!("stalwart domain deletion failed for {}: {e}", domain.name);
            }
        }
    }

    crate::db::domains::delete(&state.pool, id)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn get_domain_for_org(
    pool: &sqlx::PgPool,
    id: Uuid,
    org_id: Uuid,
) -> Result<Domain, ApiError> {
    let domain = crate::db::domains::get_by_id(pool, id)
        .await
        .map_err(ApiError::from_sqlx)?
        .ok_or(ApiError::NotFound)?;
    if domain.org_id != org_id {
        return Err(ApiError::NotFound);
    }
    Ok(domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_domain_simple() {
        assert!(is_valid_domain("example.com"));
    }

    #[test]
    fn test_is_valid_domain_subdomain() {
        assert!(is_valid_domain("mail.example.com"));
    }

    #[test]
    fn test_is_valid_domain_rejects_empty() {
        assert!(!is_valid_domain(""));
    }

    #[test]
    fn test_is_valid_domain_rejects_no_dot() {
        assert!(!is_valid_domain("localhost"));
    }

    #[test]
    fn test_is_valid_domain_rejects_spaces() {
        assert!(!is_valid_domain("example .com"));
    }

    #[test]
    fn test_is_valid_domain_rejects_http_prefix() {
        assert!(!is_valid_domain("http://example.com"));
    }

    #[test]
    fn test_is_valid_domain_rejects_https_prefix() {
        assert!(!is_valid_domain("https://example.com"));
    }

    #[test]
    fn test_is_valid_domain_rejects_leading_dot() {
        assert!(!is_valid_domain(".example.com"));
    }

    #[test]
    fn test_is_valid_domain_rejects_trailing_dot() {
        assert!(!is_valid_domain("example.com."));
    }

    #[test]
    fn test_is_valid_domain_rejects_consecutive_dots() {
        assert!(!is_valid_domain("example..com"));
    }

    #[test]
    fn test_is_valid_domain_rejects_special_chars() {
        assert!(!is_valid_domain("@#$.com"));
        assert!(!is_valid_domain("a]b.com"));
        assert!(!is_valid_domain("ex ample.com"));
    }

    #[test]
    fn test_is_valid_domain_rejects_leading_hyphen_label() {
        assert!(!is_valid_domain("-example.com"));
    }

    #[test]
    fn test_is_valid_domain_rejects_trailing_hyphen_label() {
        assert!(!is_valid_domain("example-.com"));
    }

    #[test]
    fn test_is_valid_domain_accepts_hyphenated_labels() {
        assert!(is_valid_domain("my-domain.example.com"));
    }

    #[test]
    fn test_is_valid_domain_rejects_too_long() {
        let long = format!("{}.com", "a".repeat(250));
        assert!(!is_valid_domain(&long));
    }

    #[test]
    fn test_is_valid_domain_rejects_label_over_63_chars() {
        let long_label = format!("{}.com", "a".repeat(64));
        assert!(!is_valid_domain(&long_label));
    }

    #[test]
    fn test_create_domain_request_deserialize() {
        let json = r#"{"name": "example.com"}"#;
        let req: CreateDomainRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "example.com");
    }

    #[test]
    fn test_domain_with_dns_serializes_without_dns_when_none() {
        let domain = Domain {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            name: "example.com".into(),
            status: "pending".into(),
            stalwart_principal_id: None,
            verified_at: None,
            created_at: chrono::Utc::now(),
        };
        let resp = DomainWithDns {
            domain,
            dns_records: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(!json.as_object().unwrap().contains_key("dns_records"));
    }

    #[test]
    fn test_domain_with_dns_serializes_with_dns() {
        let domain = Domain {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            name: "example.com".into(),
            status: "verified".into(),
            stalwart_principal_id: Some("p-123".into()),
            verified_at: Some(chrono::Utc::now()),
            created_at: chrono::Utc::now(),
        };
        let resp = DomainWithDns {
            domain,
            dns_records: Some(serde_json::json!({"mx": "mail.example.com"})),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.as_object().unwrap().contains_key("dns_records"));
        assert_eq!(json["name"], "example.com");
    }
}
