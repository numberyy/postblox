use axum::extract::{Query, State};
use axum::Json;
use chrono::Duration;
use serde::{Deserialize, Serialize};

use super::auth::AuthOrg;
use super::error::ApiError;
use super::AppState;
use crate::db::briefing::{InboxStats, SenderCount, SubjectCount};

#[derive(Deserialize)]
pub struct BriefingParams {
    pub period: Option<String>,
}

#[derive(Serialize)]
pub struct BriefingResponse {
    pub period: String,
    pub since: chrono::DateTime<chrono::Utc>,
    pub total_received: i64,
    pub total_sent: i64,
    pub by_inbox: Vec<InboxStats>,
    pub top_senders: Vec<SenderCount>,
    pub top_subjects: Vec<SubjectCount>,
}

fn parse_period(period: &str) -> Result<Duration, ApiError> {
    match period {
        "1h" => Ok(Duration::hours(1)),
        "6h" => Ok(Duration::hours(6)),
        "12h" => Ok(Duration::hours(12)),
        "24h" => Ok(Duration::hours(24)),
        "7d" => Ok(Duration::days(7)),
        _ => Err(ApiError::BadRequest(format!(
            "invalid period '{period}', must be one of: 1h, 6h, 12h, 24h, 7d"
        ))),
    }
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Query(params): Query<BriefingParams>,
) -> Result<Json<BriefingResponse>, ApiError> {
    let period_str = params.period.as_deref().unwrap_or("24h");
    let duration = parse_period(period_str)?;
    let since = chrono::Utc::now() - duration;

    let (by_inbox, top_senders, top_subjects) = tokio::try_join!(
        crate::db::briefing::stats_by_inbox(&state.pool, org_id, since),
        crate::db::briefing::top_senders(&state.pool, org_id, since),
        crate::db::briefing::top_subjects(&state.pool, org_id, since),
    )
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let (total_received, total_sent) = by_inbox
        .iter()
        .fold((0i64, 0i64), |(r, s), row| (r + row.received, s + row.sent));

    Ok(Json(BriefingResponse {
        period: period_str.to_string(),
        since,
        total_received,
        total_sent,
        by_inbox,
        top_senders,
        top_subjects,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn test_parse_period_valid_values() {
        assert_eq!(parse_period("1h").unwrap(), Duration::hours(1));
        assert_eq!(parse_period("6h").unwrap(), Duration::hours(6));
        assert_eq!(parse_period("12h").unwrap(), Duration::hours(12));
        assert_eq!(parse_period("24h").unwrap(), Duration::hours(24));
        assert_eq!(parse_period("7d").unwrap(), Duration::days(7));
    }

    #[test]
    fn test_parse_period_invalid_returns_error() {
        assert!(parse_period("2h").is_err());
        assert!(parse_period("30d").is_err());
        assert!(parse_period("").is_err());
        assert!(parse_period("abc").is_err());
    }

    #[test]
    fn test_briefing_response_serializes_correctly() {
        let resp = BriefingResponse {
            period: "24h".into(),
            since: Utc::now(),
            total_received: 42,
            total_sent: 15,
            by_inbox: vec![InboxStats {
                inbox_id: Uuid::new_v4(),
                inbox_email: "bot@test.com".into(),
                received: 42,
                sent: 15,
            }],
            top_senders: vec![SenderCount {
                address: "alice@example.com".into(),
                count: 8,
            }],
            top_subjects: vec![SubjectCount {
                subject: "Weekly Report".into(),
                count: 3,
            }],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["period"], "24h");
        assert_eq!(json["total_received"], 42);
        assert_eq!(json["total_sent"], 15);
        assert_eq!(json["by_inbox"].as_array().unwrap().len(), 1);
        assert_eq!(json["top_senders"][0]["address"], "alice@example.com");
        assert_eq!(json["top_subjects"][0]["subject"], "Weekly Report");
    }

    #[test]
    fn test_briefing_response_empty_results() {
        let resp = BriefingResponse {
            period: "1h".into(),
            since: Utc::now(),
            total_received: 0,
            total_sent: 0,
            by_inbox: vec![],
            top_senders: vec![],
            top_subjects: vec![],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["total_received"], 0);
        assert!(json["by_inbox"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_briefing_params_period_optional() {
        let json = r#"{}"#;
        let params: BriefingParams = serde_json::from_str(json).unwrap();
        assert!(params.period.is_none());
    }
}
