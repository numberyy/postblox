use chrono::{DateTime, Datelike, Utc};

use super::{SyncError, SyncResult};
use crate::models::LinkedAccount;

fn imap_date(dt: DateTime<Utc>) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let m = dt.month0() as usize;
    format!("{:02}-{}-{}", dt.day(), MONTHS[m], dt.year())
}

pub async fn one_shot_sync(
    pool: &sqlx::PgPool,
    account: &LinkedAccount,
    inbox: &crate::models::Inbox,
) -> Result<SyncResult, SyncError> {
    let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let tls = tokio_rustls::TlsConnector::from(std::sync::Arc::new(
        tokio_rustls::rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    ));

    let server = (account.imap_host.as_str(), account.imap_port as u16);
    let domain: tokio_rustls::rustls::pki_types::ServerName<'static> =
        tokio_rustls::rustls::pki_types::ServerName::try_from(account.imap_host.clone())
            .map_err(|e| SyncError::Connection(e.to_string()))?;

    let tcp = tokio::net::TcpStream::connect(server)
        .await
        .map_err(|e| SyncError::Connection(e.to_string()))?;

    let tls_stream = tls
        .connect(domain, tcp)
        .await
        .map_err(|e| SyncError::Connection(e.to_string()))?;

    let client = async_imap::Client::new(tls_stream);
    let mut session = client
        .login(&account.username, &account.password)
        .await
        .map_err(|e| {
            if e.0.to_string().contains("Authentication") {
                SyncError::Auth
            } else {
                SyncError::Protocol(e.0.to_string())
            }
        })?;

    session
        .select("INBOX")
        .await
        .map_err(|e| SyncError::Protocol(e.to_string()))?;

    let search_query = match account.last_sync_at {
        Some(dt) => format!("SINCE {}", imap_date(dt)),
        None => "ALL".into(),
    };

    let uids = session
        .search(&search_query)
        .await
        .map_err(|e| SyncError::Protocol(e.to_string()))?;

    let mut result = SyncResult {
        fetched: 0,
        stored: 0,
        skipped: 0,
    };

    if uids.is_empty() {
        if let Err(e) = session.logout().await {
            tracing::warn!("IMAP logout failed: {e}");
        }
        return Ok(result);
    }

    // Cap at 500 UIDs per sync to bound memory usage
    const MAX_UIDS_PER_SYNC: usize = 500;
    let mut uid_vec: Vec<_> = uids.into_iter().collect();
    uid_vec.sort_unstable();
    if uid_vec.len() > MAX_UIDS_PER_SYNC {
        tracing::info!(
            total = uid_vec.len(),
            cap = MAX_UIDS_PER_SYNC,
            "capping IMAP fetch to newest UIDs"
        );
        uid_vec = uid_vec[uid_vec.len() - MAX_UIDS_PER_SYNC..].to_vec();
    }

    let uid_list: String = uid_vec
        .iter()
        .map(|u| u.to_string())
        .collect::<Vec<_>>()
        .join(",");

    use futures::StreamExt;
    let mut messages = session
        .fetch(&uid_list, "RFC822")
        .await
        .map_err(|e| SyncError::Protocol(e.to_string()))?;

    struct ParsedMsg {
        message_id: Option<String>,
        create_msg: crate::models::CreateMessage,
    }
    let mut parsed_msgs: Vec<ParsedMsg> = Vec::new();
    while let Some(msg_result) = messages.next().await {
        let fetch = msg_result.map_err(|e| SyncError::Protocol(e.to_string()))?;
        let body = match fetch.body() {
            Some(b) => b,
            None => continue,
        };
        result.fetched += 1;

        let parsed = match crate::mail::parser::parse(body) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("failed to parse IMAP message: {e}");
                result.skipped += 1;
                continue;
            }
        };

        let mid = parsed.message_id.clone();
        let create_msg = crate::mail::parsed_to_create_message(&parsed, inbox.id, None, None);
        parsed_msgs.push(ParsedMsg {
            message_id: mid,
            create_msg,
        });
    }
    drop(messages);

    let mids: Vec<&str> = parsed_msgs
        .iter()
        .filter_map(|m| m.message_id.as_deref())
        .collect();
    let existing_mids =
        crate::db::messages::find_existing_message_ids(pool, inbox.id, &mids).await?;

    for msg in parsed_msgs {
        if let Some(ref mid) = msg.message_id {
            if existing_mids.contains(mid) {
                result.skipped += 1;
                continue;
            }
        }
        crate::db::messages::create(pool, &msg.create_msg).await?;
        result.stored += 1;
    }

    let now = Utc::now();
    let total = account.message_count + result.stored as i32;
    crate::db::linked_accounts::complete_sync(pool, account.id, total, now).await?;

    if let Err(e) = session.logout().await {
        tracing::warn!("IMAP logout failed: {e}");
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_imap_date_normal() {
        let dt = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
        assert_eq!(imap_date(dt), "15-Jan-2026");
    }

    #[test]
    fn test_imap_date_single_digit_day() {
        let dt = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        assert_eq!(imap_date(dt), "01-Mar-2026");
    }

    #[test]
    fn test_imap_date_december() {
        let dt = Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 59).unwrap();
        assert_eq!(imap_date(dt), "31-Dec-2026");
    }

    #[test]
    fn test_imap_date_february() {
        let dt = Utc.with_ymd_and_hms(2024, 2, 29, 12, 0, 0).unwrap();
        assert_eq!(imap_date(dt), "29-Feb-2024");
    }

    #[test]
    fn test_imap_date_all_months() {
        let months = [
            (1, "Jan"),
            (2, "Feb"),
            (3, "Mar"),
            (4, "Apr"),
            (5, "May"),
            (6, "Jun"),
            (7, "Jul"),
            (8, "Aug"),
            (9, "Sep"),
            (10, "Oct"),
            (11, "Nov"),
            (12, "Dec"),
        ];
        for (m, name) in months {
            let dt = Utc.with_ymd_and_hms(2026, m, 10, 0, 0, 0).unwrap();
            assert!(
                imap_date(dt).contains(name),
                "month {m} should produce {name}"
            );
        }
    }

    #[test]
    fn test_imap_date_epoch() {
        let dt = Utc.with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(imap_date(dt), "01-Jan-1970");
    }

    #[test]
    fn test_imap_date_year_2000() {
        let dt = Utc.with_ymd_and_hms(2000, 6, 15, 0, 0, 0).unwrap();
        assert_eq!(imap_date(dt), "15-Jun-2000");
    }
}
