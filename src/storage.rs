use std::path::{Path, PathBuf};

use tokio::fs;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("attachment too large: {size} bytes exceeds limit of {limit} bytes")]
    TooLarge { size: u64, limit: u64 },
    #[error("invalid storage key: {0}")]
    InvalidKey(String),
}

fn validate_filename(name: &str) -> Result<(), StorageError> {
    if name.contains("..") || name.contains('/') || name.contains('\\') || name.is_empty() {
        return Err(StorageError::InvalidKey(name.to_string()));
    }
    Ok(())
}

fn storage_path(base: &Path, message_id: &str, filename: &str) -> Result<PathBuf, StorageError> {
    validate_filename(message_id)?;
    validate_filename(filename)?;
    Ok(base.join(message_id).join(filename))
}

pub async fn store_attachment(
    base_path: &Path,
    message_id: &str,
    filename: &str,
    data: &[u8],
    max_size: u64,
) -> Result<String, StorageError> {
    if data.len() as u64 > max_size {
        return Err(StorageError::TooLarge {
            size: data.len() as u64,
            limit: max_size,
        });
    }

    let path = storage_path(base_path, message_id, filename)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    fs::write(&path, data).await?;

    Ok(format!("{message_id}/{filename}"))
}

fn validate_storage_key(storage_key: &str) -> Result<(), StorageError> {
    let Some((msg_id, filename)) = storage_key.split_once('/') else {
        return Err(StorageError::InvalidKey(storage_key.to_string()));
    };
    validate_filename(msg_id)?;
    validate_filename(filename)?;
    Ok(())
}

pub async fn read_attachment(base_path: &Path, storage_key: &str) -> Result<Vec<u8>, StorageError> {
    validate_storage_key(storage_key)?;
    let path = base_path.join(storage_key);
    Ok(fs::read(&path).await?)
}

pub async fn delete_attachment(base_path: &Path, storage_key: &str) -> Result<(), StorageError> {
    validate_storage_key(storage_key)?;

    let path = base_path.join(storage_key);
    match fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_store_and_read_attachment() {
        let tmp = TempDir::new().unwrap();
        let data = b"hello world";
        let key = store_attachment(tmp.path(), "msg-123", "file.txt", data, 1024)
            .await
            .unwrap();

        assert_eq!(key, "msg-123/file.txt");

        let read = read_attachment(tmp.path(), &key).await.unwrap();
        assert_eq!(read, data);
    }

    #[tokio::test]
    async fn test_store_attachment_too_large() {
        let tmp = TempDir::new().unwrap();
        let data = vec![0u8; 100];
        let result = store_attachment(tmp.path(), "msg-1", "big.bin", &data, 50).await;

        assert!(matches!(
            result,
            Err(StorageError::TooLarge {
                size: 100,
                limit: 50
            })
        ));
    }

    #[tokio::test]
    async fn test_store_attachment_path_traversal_rejected() {
        let tmp = TempDir::new().unwrap();
        let result = store_attachment(tmp.path(), "msg-1", "../etc/passwd", b"x", 1024).await;
        assert!(matches!(result, Err(StorageError::InvalidKey(_))));

        let result2 = store_attachment(tmp.path(), "msg-1", "sub/file.txt", b"x", 1024).await;
        assert!(matches!(result2, Err(StorageError::InvalidKey(_))));
    }

    #[tokio::test]
    async fn test_store_attachment_empty_filename_rejected() {
        let tmp = TempDir::new().unwrap();
        let result = store_attachment(tmp.path(), "msg-1", "", b"x", 1024).await;
        assert!(matches!(result, Err(StorageError::InvalidKey(_))));
    }

    #[tokio::test]
    async fn test_read_attachment_invalid_key() {
        let tmp = TempDir::new().unwrap();
        let result = read_attachment(tmp.path(), "no-slash").await;
        assert!(matches!(result, Err(StorageError::InvalidKey(_))));
    }

    #[tokio::test]
    async fn test_read_attachment_not_found() {
        let tmp = TempDir::new().unwrap();
        let result = read_attachment(tmp.path(), "msg-1/missing.txt").await;
        assert!(matches!(result, Err(StorageError::Io(_))));
    }

    #[tokio::test]
    async fn test_delete_attachment() {
        let tmp = TempDir::new().unwrap();
        store_attachment(tmp.path(), "msg-1", "del.txt", b"data", 1024)
            .await
            .unwrap();

        delete_attachment(tmp.path(), "msg-1/del.txt")
            .await
            .unwrap();
        let result = read_attachment(tmp.path(), "msg-1/del.txt").await;
        assert!(matches!(result, Err(StorageError::Io(_))));
    }

    #[tokio::test]
    async fn test_delete_attachment_nonexistent_ok() {
        let tmp = TempDir::new().unwrap();
        delete_attachment(tmp.path(), "msg-1/nope.txt")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_store_attachment_zero_bytes() {
        let tmp = TempDir::new().unwrap();
        let key = store_attachment(tmp.path(), "msg-1", "empty.txt", b"", 1024)
            .await
            .unwrap();
        let read = read_attachment(tmp.path(), &key).await.unwrap();
        assert!(read.is_empty());
    }

    #[tokio::test]
    async fn test_store_attachment_exactly_at_limit() {
        let tmp = TempDir::new().unwrap();
        let data = vec![0u8; 100];
        let key = store_attachment(tmp.path(), "msg-1", "exact.bin", &data, 100)
            .await
            .unwrap();
        let read = read_attachment(tmp.path(), &key).await.unwrap();
        assert_eq!(read.len(), 100);
    }
}
