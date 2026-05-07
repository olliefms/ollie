pub mod extract_store;
pub mod shard;

use crate::error::AppError;
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs;

pub struct BlobStore {
    base: String,
}

impl BlobStore {
    pub fn new(base: &str) -> Self {
        Self { base: base.to_string() }
    }

    pub fn path_for(&self, checksum: &str) -> PathBuf {
        shard::shard_path(&self.base, checksum)
    }

    pub async fn write(&self, data: &Bytes) -> Result<String, AppError> {
        let checksum = compute_checksum(data);
        let path = self.path_for(&checksum);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, data).await?;
        Ok(checksum)
    }

    pub async fn read(&self, checksum: &str) -> Result<Bytes, AppError> {
        let path = self.path_for(checksum);
        match fs::read(&path).await {
            Ok(data) => Ok(Bytes::from(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(AppError::NotFound),
            Err(e) => Err(AppError::Internal(e.to_string())),
        }
    }

    pub async fn delete(&self, checksum: &str) -> Result<(), AppError> {
        let path = self.path_for(checksum);
        match fs::remove_file(&path).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::Internal(e.to_string())),
        }
    }

    pub async fn exists(&self, checksum: &str) -> bool {
        self.path_for(checksum).exists()
    }
}

pub fn compute_checksum(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn temp_store() -> (BlobStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = BlobStore::new(dir.path().to_str().unwrap());
        (store, dir)
    }

    #[tokio::test]
    async fn test_write_and_read_roundtrip() {
        let (store, _dir) = temp_store().await;
        let data = Bytes::from("hello world");
        let checksum = store.write(&data).await.unwrap();
        let read_back = store.read(&checksum).await.unwrap();
        assert_eq!(data, read_back);
    }

    #[tokio::test]
    async fn test_checksum_matches_sha256_of_hello() {
        let data = b"hello";
        let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert_eq!(compute_checksum(data), expected);
    }

    #[tokio::test]
    async fn test_exists_false_before_write() {
        let (store, _dir) = temp_store().await;
        assert!(!store.exists("deadbeef00000000deadbeef00000000deadbeef00000000deadbeef00000000").await);
    }

    #[tokio::test]
    async fn test_delete_removes_file() {
        let (store, _dir) = temp_store().await;
        let data = Bytes::from("to delete");
        let checksum = store.write(&data).await.unwrap();
        assert!(store.exists(&checksum).await);
        store.delete(&checksum).await.unwrap();
        assert!(!store.exists(&checksum).await);
    }

    #[tokio::test]
    async fn test_read_missing_returns_not_found() {
        let (store, _dir) = temp_store().await;
        let result = store
            .read("0000000000000000000000000000000000000000000000000000000000000000")
            .await;
        assert!(matches!(result, Err(AppError::NotFound)));
    }
}
