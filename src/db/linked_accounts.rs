use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{CreateLinkedAccount, LinkedAccount};

#[derive(Debug, thiserror::Error)]
pub enum LinkedAccountError {
    #[error("encryption key required to store IMAP passwords")]
    NoEncryptionKey,
    #[error("encryption failed: {0}")]
    Encryption(String),
    #[error("{0}")]
    Database(#[from] sqlx::Error),
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err("odd-length hex string".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

pub(crate) fn encrypt_password(
    key: &[u8; 32],
    plaintext: &str,
) -> Result<(String, String), String> {
    use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
    use aes_gcm::Aes256Gcm;

    let cipher = Aes256Gcm::new(key.into());
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok((hex_encode(&ciphertext), hex_encode(&nonce)))
}

pub(crate) fn decrypt_password(
    key: &[u8; 32],
    ciphertext_hex: &str,
    nonce_hex: &str,
) -> Result<String, String> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};

    let cipher = Aes256Gcm::new(key.into());
    let ciphertext = hex_decode(ciphertext_hex)?;
    let nonce_bytes = hex_decode(nonce_hex)?;
    if nonce_bytes.len() != 12 {
        return Err(format!(
            "invalid nonce length: expected 12, got {}",
            nonce_bytes.len()
        ));
    }
    let nonce = Nonce::from_slice(&nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|e| e.to_string())?;
    String::from_utf8(plaintext).map_err(|e| e.to_string())
}

const LINKED_ACCOUNT_COLUMNS: &str =
    "id, inbox_id, org_id, provider, imap_host, imap_port, username, \
     password, password_nonce, last_sync_at, sync_status, message_count, created_at";

pub async fn create(
    pool: &PgPool,
    account: &CreateLinkedAccount,
    encryption_key: Option<&[u8; 32]>,
) -> Result<LinkedAccount, LinkedAccountError> {
    let key = encryption_key.ok_or(LinkedAccountError::NoEncryptionKey)?;
    let (encrypted_password, nonce) =
        encrypt_password(key, &account.password).map_err(LinkedAccountError::Encryption)?;
    let port = account.imap_port.unwrap_or(993);

    sqlx::query_as(&format!(
        "INSERT INTO linked_accounts \
         (inbox_id, org_id, imap_host, imap_port, username, password, password_nonce) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING {LINKED_ACCOUNT_COLUMNS}",
    ))
    .bind(account.inbox_id)
    .bind(account.org_id)
    .bind(&account.imap_host)
    .bind(port)
    .bind(&account.username)
    .bind(&encrypted_password)
    .bind(&nonce)
    .fetch_one(pool)
    .await
    .map_err(LinkedAccountError::Database)
}

pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<LinkedAccount>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {LINKED_ACCOUNT_COLUMNS} FROM linked_accounts WHERE id = $1",
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn list_by_org(pool: &PgPool, org_id: Uuid) -> Result<Vec<LinkedAccount>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {LINKED_ACCOUNT_COLUMNS} \
         FROM linked_accounts WHERE org_id = $1 ORDER BY created_at DESC",
    ))
    .bind(org_id)
    .fetch_all(pool)
    .await
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM linked_accounts WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_sync_status(
    pool: &PgPool,
    id: Uuid,
    status: crate::sync::SyncStatus,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE linked_accounts SET sync_status = $2 WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn complete_sync(
    pool: &PgPool,
    id: Uuid,
    message_count: i32,
    last_sync_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE linked_accounts SET sync_status = 'idle', message_count = $2, last_sync_at = $3 \
         WHERE id = $1",
    )
    .bind(id)
    .bind(message_count)
    .bind(last_sync_at)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [0x42u8; 32];
        let password = "my-secret-password";
        let (encrypted, nonce) = encrypt_password(&key, password).unwrap();
        assert_ne!(encrypted, password);
        let decrypted = decrypt_password(&key, &encrypted, &nonce).unwrap();
        assert_eq!(decrypted, password);
    }

    #[test]
    fn test_encrypt_different_nonces() {
        let key = [0x42u8; 32];
        let (enc1, nonce1) = encrypt_password(&key, "password").unwrap();
        let (enc2, nonce2) = encrypt_password(&key, "password").unwrap();
        assert_ne!(nonce1, nonce2);
        assert_ne!(enc1, enc2);
    }

    #[test]
    fn test_decrypt_wrong_key_fails() {
        let key1 = [0x42u8; 32];
        let key2 = [0x43u8; 32];
        let (encrypted, nonce) = encrypt_password(&key1, "password").unwrap();
        assert!(decrypt_password(&key2, &encrypted, &nonce).is_err());
    }

    #[test]
    fn test_encrypt_empty_password() {
        let key = [0x42u8; 32];
        let (encrypted, nonce) = encrypt_password(&key, "").unwrap();
        let decrypted = decrypt_password(&key, &encrypted, &nonce).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn test_encrypt_unicode_password() {
        let key = [0x42u8; 32];
        let password = "пароль-密码-パスワード";
        let (encrypted, nonce) = encrypt_password(&key, password).unwrap();
        let decrypted = decrypt_password(&key, &encrypted, &nonce).unwrap();
        assert_eq!(decrypted, password);
    }

    #[test]
    fn test_hex_encode_decode_roundtrip() {
        let data = b"hello world";
        let hex = hex_encode(data);
        let decoded = hex_decode(&hex).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_hex_decode_invalid() {
        assert!(hex_decode("zz").is_err());
        assert!(hex_decode("abc").is_err());
    }

    #[test]
    fn test_decrypt_invalid_nonce_length() {
        let key = [0x42u8; 32];
        let (encrypted, _) = encrypt_password(&key, "test").unwrap();
        assert!(decrypt_password(&key, &encrypted, "aabbccdd").is_err());
    }

    #[test]
    fn test_decrypt_corrupted_ciphertext() {
        let key = [0x42u8; 32];
        let (_, nonce) = encrypt_password(&key, "test").unwrap();
        assert!(decrypt_password(&key, "deadbeef", &nonce).is_err());
    }

    #[tokio::test]
    #[ignore] // requires running Postgres
    async fn test_linked_account_create_and_get() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA Org")
            .await
            .unwrap();
        let email = format!("la-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Relay,
        )
        .await
        .unwrap();

        let key = [0x42u8; 32];
        let input = CreateLinkedAccount {
            inbox_id: inbox.id,
            org_id: org.id,
            imap_host: "imap.example.com".into(),
            imap_port: Some(993),
            username: "user@example.com".into(),
            password: "enc_pass".into(),
        };
        let acct = create(&pool, &input, Some(&key)).await.unwrap();
        assert_eq!(acct.inbox_id, inbox.id);
        assert_eq!(acct.org_id, org.id);
        assert_eq!(acct.imap_host, "imap.example.com");
        assert_eq!(acct.imap_port, 993);
        assert_eq!(acct.sync_status, crate::sync::SyncStatus::Idle);
        assert_eq!(acct.message_count, 0);
        assert!(acct.last_sync_at.is_none());
        assert!(acct.password_nonce.is_some());

        let fetched = get_by_id(&pool, acct.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, acct.id);
        let decrypted = decrypt_password(
            &key,
            &fetched.password,
            fetched.password_nonce.as_deref().unwrap(),
        )
        .unwrap();
        assert_eq!(decrypted, "enc_pass");
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_create_no_key_fails() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA NoKey Org")
            .await
            .unwrap();
        let email = format!("la-nokey-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Relay,
        )
        .await
        .unwrap();

        let input = CreateLinkedAccount {
            inbox_id: inbox.id,
            org_id: org.id,
            imap_host: "imap.example.com".into(),
            imap_port: None,
            username: "user".into(),
            password: "pw".into(),
        };
        let result = create(&pool, &input, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("encryption key"));
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_defaults() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA Default Org")
            .await
            .unwrap();
        let email = format!("la-def-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Relay,
        )
        .await
        .unwrap();

        let key = [0xaa; 32];
        let input = CreateLinkedAccount {
            inbox_id: inbox.id,
            org_id: org.id,
            imap_host: "imap.test.com".into(),
            imap_port: None,
            username: "user".into(),
            password: "pw".into(),
        };
        let acct = create(&pool, &input, Some(&key)).await.unwrap();
        assert_eq!(acct.imap_port, 993);
        assert_eq!(acct.provider, "imap");
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_get_nonexistent_returns_none() {
        let pool = crate::db::test_pool().await;
        let result = get_by_id(&pool, Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_delete_nonexistent_returns_false() {
        let pool = crate::db::test_pool().await;
        assert!(!delete(&pool, Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_duplicate_inbox_fails() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA Dup Org")
            .await
            .unwrap();
        let email = format!("la-dup-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Relay,
        )
        .await
        .unwrap();
        let key = [0xbb; 32];
        let input = CreateLinkedAccount {
            inbox_id: inbox.id,
            org_id: org.id,
            imap_host: "imap.one.com".into(),
            imap_port: None,
            username: "u".into(),
            password: "p".into(),
        };
        create(&pool, &input, Some(&key)).await.unwrap();
        assert!(create(&pool, &input, Some(&key)).await.is_err());
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_list_by_org() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA List Org")
            .await
            .unwrap();
        let key = [0xcc; 32];
        for i in 0..2 {
            let email = format!("la-list-{i}-{}@example.com", Uuid::new_v4());
            let inbox = crate::db::inboxes::create(
                &pool,
                org.id,
                &email,
                None,
                crate::models::InboxType::Relay,
            )
            .await
            .unwrap();
            let input = CreateLinkedAccount {
                inbox_id: inbox.id,
                org_id: org.id,
                imap_host: format!("imap{i}.example.com"),
                imap_port: None,
                username: format!("user{i}"),
                password: "pw".into(),
            };
            create(&pool, &input, Some(&key)).await.unwrap();
        }
        let accounts = list_by_org(&pool, org.id).await.unwrap();
        assert_eq!(accounts.len(), 2);
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_list_by_org_empty() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA Empty Org")
            .await
            .unwrap();
        let accounts = list_by_org(&pool, org.id).await.unwrap();
        assert!(accounts.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_list_by_org_isolation() {
        let pool = crate::db::test_pool().await;
        let org1 = crate::db::organizations::create(&pool, "LA Iso Org1")
            .await
            .unwrap();
        let org2 = crate::db::organizations::create(&pool, "LA Iso Org2")
            .await
            .unwrap();
        let key = [0xdd; 32];
        let email = format!("la-iso1-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org1.id,
            &email,
            None,
            crate::models::InboxType::Relay,
        )
        .await
        .unwrap();
        let input = CreateLinkedAccount {
            inbox_id: inbox.id,
            org_id: org1.id,
            imap_host: "imap.one.com".into(),
            imap_port: None,
            username: "u".into(),
            password: "p".into(),
        };
        create(&pool, &input, Some(&key)).await.unwrap();
        let accounts = list_by_org(&pool, org2.id).await.unwrap();
        assert!(accounts.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_delete() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA Del Org")
            .await
            .unwrap();
        let key = [0xee; 32];
        let email = format!("la-del-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Relay,
        )
        .await
        .unwrap();
        let input = CreateLinkedAccount {
            inbox_id: inbox.id,
            org_id: org.id,
            imap_host: "imap.del.com".into(),
            imap_port: None,
            username: "u".into(),
            password: "p".into(),
        };
        let acct = create(&pool, &input, Some(&key)).await.unwrap();
        assert!(delete(&pool, acct.id).await.unwrap());
        assert!(get_by_id(&pool, acct.id).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_set_sync_status() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA Sync Org")
            .await
            .unwrap();
        let key = [0xff; 32];
        let email = format!("la-sync-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Relay,
        )
        .await
        .unwrap();
        let input = CreateLinkedAccount {
            inbox_id: inbox.id,
            org_id: org.id,
            imap_host: "imap.sync.com".into(),
            imap_port: None,
            username: "u".into(),
            password: "p".into(),
        };
        let acct = create(&pool, &input, Some(&key)).await.unwrap();
        set_sync_status(&pool, acct.id, crate::sync::SyncStatus::Syncing)
            .await
            .unwrap();
        let fetched = get_by_id(&pool, acct.id).await.unwrap().unwrap();
        assert_eq!(fetched.sync_status, crate::sync::SyncStatus::Syncing);
        let now = Utc::now();
        complete_sync(&pool, acct.id, 15, now).await.unwrap();
        let fetched = get_by_id(&pool, acct.id).await.unwrap().unwrap();
        assert_eq!(fetched.sync_status, crate::sync::SyncStatus::Idle);
        assert_eq!(fetched.message_count, 15);
        assert!(fetched.last_sync_at.is_some());
    }
}
