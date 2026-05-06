//! AES-256-GCM file backend for [`SecretStore`].
//!
//! File format (little-endian byte stream, all fields fixed-width):
//!   [ 1 byte version = 0x01 ]
//!   [ 16 byte Argon2id salt ]
//!   [ 12 byte AES-GCM nonce ]
//!   [ ciphertext || 16-byte AEAD tag ]
//!
//! Plaintext is JSON: `{ "<account_uuid>": "<secret>" }`. Whole-file
//! encryption is fine for the bounded account count this project
//! targets (single user, ≤ a handful of accounts).
//!
//! Writes go through a temp file + atomic rename so a crash mid-write
//! cannot leave a partially encrypted file on disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use async_trait::async_trait;
use rand::RngCore;
use tokio::sync::Mutex;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use super::{Secret, SecretError, SecretStore};

const VERSION: u8 = 0x01;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// Argon2id parameters used to derive the AES-256 key from the user's
/// passphrase. The OWASP-recommended defaults take ~80 ms on a modern
/// laptop; tests use [`KdfParams::insecure_for_tests`] so the suite
/// stays under a second.
#[derive(Debug, Clone, Copy)]
pub struct KdfParams {
    pub mem_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        // OWASP 2023 minimum for Argon2id: 19 MiB, 2 iters, 1 lane.
        Self {
            mem_kib: 19_456,
            iterations: 2,
            parallelism: 1,
        }
    }
}

impl KdfParams {
    /// Trivially-cheap params used in tests. Do not use in production:
    /// the work factor is below the brute-force threshold.
    pub fn insecure_for_tests() -> Self {
        Self {
            mem_kib: Params::MIN_M_COST,
            iterations: Params::MIN_T_COST,
            parallelism: 1,
        }
    }

    fn build(&self) -> Result<Argon2<'static>, SecretError> {
        let p = Params::new(
            self.mem_kib,
            self.iterations,
            self.parallelism,
            Some(KEY_LEN),
        )
        .map_err(|e| SecretError::Crypto(format!("argon2 params: {e}")))?;
        Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, p))
    }
}

/// File-backed secret store. Cheap to clone (`Arc` inside).
#[derive(Clone)]
pub struct FileSecretStore {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    passphrase: Zeroizing<String>,
    kdf: KdfParams,
    /// Serialises the read-modify-write cycle so concurrent puts don't
    /// drop each other's writes.
    write_lock: Mutex<()>,
}

impl FileSecretStore {
    /// Open (or create) a secret store at `path`. The file is not read
    /// at open time — it is read lazily on first access. The
    /// passphrase is used to derive the AES-256 key via Argon2id.
    pub fn new(path: impl Into<PathBuf>, passphrase: impl Into<String>) -> Self {
        Self::with_params(path, passphrase, KdfParams::default())
    }

    pub fn with_params(
        path: impl Into<PathBuf>,
        passphrase: impl Into<String>,
        kdf: KdfParams,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                path: path.into(),
                passphrase: Zeroizing::new(passphrase.into()),
                kdf,
                write_lock: Mutex::new(()),
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    async fn read_map(&self) -> Result<BTreeMap<Uuid, String>, SecretError> {
        let bytes = match tokio::fs::read(&self.inner.path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(SecretError::Io(e)),
        };
        if bytes.is_empty() {
            return Ok(BTreeMap::new());
        }
        decrypt(&bytes, &self.inner.passphrase, &self.inner.kdf)
    }

    async fn write_map(&self, map: &BTreeMap<Uuid, String>) -> Result<(), SecretError> {
        if let Some(parent) = self.inner.path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }
        let payload = encrypt(map, &self.inner.passphrase, &self.inner.kdf)?;
        let tmp = tmp_path(&self.inner.path);
        tokio::fs::write(&tmp, &payload).await?;
        // atomic on linux + macOS for same-fs renames
        tokio::fs::rename(&tmp, &self.inner.path).await?;
        Ok(())
    }
}

#[async_trait]
impl SecretStore for FileSecretStore {
    async fn put(&self, account_id: Uuid, secret: Secret) -> Result<(), SecretError> {
        let _guard = self.inner.write_lock.lock().await;
        let mut map = self.read_map().await?;
        map.insert(account_id, secret.as_str().to_string());
        self.write_map(&map).await?;
        // Wipe the in-memory copy of every secret before the map is
        // freed so passwords don't linger on the heap.
        for v in map.values_mut() {
            v.zeroize();
        }
        Ok(())
    }

    async fn get(&self, account_id: Uuid) -> Result<Option<Secret>, SecretError> {
        let map = self.read_map().await?;
        Ok(map.get(&account_id).map(|s| Zeroizing::new(s.clone())))
    }

    async fn delete(&self, account_id: Uuid) -> Result<(), SecretError> {
        let _guard = self.inner.write_lock.lock().await;
        let mut map = self.read_map().await?;
        if map.remove(&account_id).is_some() {
            self.write_map(&map).await?;
        }
        Ok(())
    }
}

fn tmp_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

fn derive_key(
    passphrase: &str,
    salt: &[u8],
    kdf: &KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>, SecretError> {
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    kdf.build()?
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut())
        .map_err(|e| SecretError::Crypto(format!("argon2: {e}")))?;
    Ok(key)
}

fn encrypt(
    map: &BTreeMap<Uuid, String>,
    passphrase: &str,
    kdf: &KdfParams,
) -> Result<Vec<u8>, SecretError> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    let mut rng = rand::thread_rng();
    rng.fill_bytes(&mut salt);
    rng.fill_bytes(&mut nonce_bytes);

    let key = derive_key(passphrase, &salt, kdf)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key.as_ref()));
    let plaintext =
        serde_json::to_vec(map).map_err(|e| SecretError::Decode(format!("serialise map: {e}")))?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_ref())
        .map_err(|e| SecretError::Crypto(format!("encrypt: {e}")))?;

    let mut out = Vec::with_capacity(1 + SALT_LEN + NONCE_LEN + ciphertext.len());
    out.push(VERSION);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt(
    bytes: &[u8],
    passphrase: &str,
    kdf: &KdfParams,
) -> Result<BTreeMap<Uuid, String>, SecretError> {
    if bytes.len() < 1 + SALT_LEN + NONCE_LEN + 16 {
        return Err(SecretError::Decode("file too short".into()));
    }
    if bytes[0] != VERSION {
        return Err(SecretError::Decode(format!(
            "unknown version 0x{:02x}",
            bytes[0]
        )));
    }
    let salt = &bytes[1..1 + SALT_LEN];
    let nonce = &bytes[1 + SALT_LEN..1 + SALT_LEN + NONCE_LEN];
    let ct = &bytes[1 + SALT_LEN + NONCE_LEN..];
    let key = derive_key(passphrase, salt, kdf)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key.as_ref()));
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| SecretError::Crypto("aead authentication failed".into()))?;
    serde_json::from_slice(&plaintext)
        .map_err(|e| SecretError::Decode(format!("plaintext json: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store(dir: &TempDir, pass: &str) -> FileSecretStore {
        FileSecretStore::with_params(
            dir.path().join("secrets.bin"),
            pass,
            KdfParams::insecure_for_tests(),
        )
    }

    #[tokio::test]
    async fn missing_file_returns_none_for_get() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir, "pass");
        let id = Uuid::new_v4();
        assert!(s.get(id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn put_then_get_round_trip() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir, "correct horse battery staple");
        let id = Uuid::new_v4();
        s.put(id, Zeroizing::new("hunter2".into())).await.unwrap();
        let got = s.get(id).await.unwrap().unwrap();
        assert_eq!(got.as_str(), "hunter2");
    }

    #[tokio::test]
    async fn delete_removes_entry_but_leaves_others() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir, "p");
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        s.put(a, Zeroizing::new("aa".into())).await.unwrap();
        s.put(b, Zeroizing::new("bb".into())).await.unwrap();
        s.delete(a).await.unwrap();
        assert!(s.get(a).await.unwrap().is_none());
        assert_eq!(s.get(b).await.unwrap().unwrap().as_str(), "bb");
    }

    #[tokio::test]
    async fn delete_unknown_id_is_no_op() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir, "p");
        s.delete(Uuid::new_v4()).await.unwrap();
    }

    #[tokio::test]
    async fn wrong_passphrase_fails_to_decrypt() {
        let dir = TempDir::new().unwrap();
        let id = Uuid::new_v4();
        {
            let s = store(&dir, "good");
            s.put(id, Zeroizing::new("secret".into())).await.unwrap();
        }
        let bad = store(&dir, "wrong");
        let err = bad.get(id).await.unwrap_err();
        assert!(matches!(err, SecretError::Crypto(_)));
    }

    #[tokio::test]
    async fn tampered_ciphertext_fails_aead() {
        let dir = TempDir::new().unwrap();
        let id = Uuid::new_v4();
        let path = dir.path().join("secrets.bin");
        let kdf = KdfParams::insecure_for_tests();
        {
            let s = FileSecretStore::with_params(&path, "pass", kdf);
            s.put(id, Zeroizing::new("v".into())).await.unwrap();
        }
        // Flip a single bit in the body.
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        std::fs::write(&path, &bytes).unwrap();

        let s = FileSecretStore::with_params(&path, "pass", kdf);
        let err = s.get(id).await.unwrap_err();
        assert!(matches!(err, SecretError::Crypto(_)));
    }

    #[tokio::test]
    async fn corrupted_header_returns_decode_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("secrets.bin");
        std::fs::write(&path, b"\xff\xff\xff\xff").unwrap();
        let s = FileSecretStore::with_params(&path, "pass", KdfParams::insecure_for_tests());
        let err = s.get(Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(err, SecretError::Decode(_)));
    }

    #[tokio::test]
    async fn put_overwrites_existing_value() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir, "p");
        let id = Uuid::new_v4();
        s.put(id, Zeroizing::new("v1".into())).await.unwrap();
        s.put(id, Zeroizing::new("v2".into())).await.unwrap();
        assert_eq!(s.get(id).await.unwrap().unwrap().as_str(), "v2");
    }

    #[tokio::test]
    async fn concurrent_puts_all_persist() {
        let dir = TempDir::new().unwrap();
        let s = store(&dir, "p");
        let mut handles = Vec::new();
        let ids: Vec<Uuid> = (0..16).map(|_| Uuid::new_v4()).collect();
        for id in ids.iter().copied() {
            let s2 = s.clone();
            handles.push(tokio::spawn(async move {
                s2.put(id, Zeroizing::new(format!("v-{id}"))).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        for id in ids {
            assert_eq!(
                s.get(id).await.unwrap().unwrap().as_str(),
                format!("v-{id}")
            );
        }
    }

    #[tokio::test]
    async fn parent_dir_is_created_lazily() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("a").join("b").join("c").join("secrets.bin");
        let s = FileSecretStore::with_params(&nested, "pass", KdfParams::insecure_for_tests());
        s.put(Uuid::new_v4(), Zeroizing::new("v".into()))
            .await
            .unwrap();
        assert!(nested.exists());
    }

    #[tokio::test]
    async fn nonces_differ_between_writes() {
        // GCM is catastrophic if a nonce is reused with the same key.
        let dir = TempDir::new().unwrap();
        let s = store(&dir, "p");
        let id = Uuid::new_v4();
        s.put(id, Zeroizing::new("v1".into())).await.unwrap();
        let first = std::fs::read(dir.path().join("secrets.bin")).unwrap();
        s.put(id, Zeroizing::new("v1".into())).await.unwrap();
        let second = std::fs::read(dir.path().join("secrets.bin")).unwrap();
        let nonce_range = 1 + SALT_LEN..1 + SALT_LEN + NONCE_LEN;
        assert_ne!(first[nonce_range.clone()], second[nonce_range]);
    }
}
