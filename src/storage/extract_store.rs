use crate::{ai::extract::Extractable, error::AppError, storage::shard::shard_path};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ExtractCache {
    Text { content: String },
    ScannedPdf { content: String, raw_text: String },
}

pub enum ExtractForQuery {
    Text(String),
    ScannedPdf(String),
}

fn extract_path(base_dir: &str, checksum: &str) -> PathBuf {
    shard_path(base_dir, checksum).with_extension("json")
}

pub async fn read_extract(base_dir: &str, checksum: &str) -> Option<ExtractForQuery> {
    let bytes = fs::read(extract_path(base_dir, checksum)).await.ok()?;
    match serde_json::from_slice::<ExtractCache>(&bytes).ok()? {
        ExtractCache::Text { content } => Some(ExtractForQuery::Text(content)),
        ExtractCache::ScannedPdf { raw_text, .. } => Some(ExtractForQuery::ScannedPdf(raw_text)),
    }
}

pub async fn write_extract(base_dir: &str, checksum: &str, extractable: &Extractable) -> Result<(), AppError> {
    let cache = match extractable {
        Extractable::Text(text) => ExtractCache::Text { content: text.clone() },
        Extractable::ScannedPdf(_, raw_text) => ExtractCache::ScannedPdf {
            content: raw_text.clone(),
            raw_text: raw_text.clone(),
        },
        Extractable::ImageBytes(_) | Extractable::Unsupported => return Ok(()),
    };

    let path = extract_path(base_dir, checksum);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let json = serde_json::to_vec(&cache).map_err(|e| AppError::Internal(e.to_string()))?;
    fs::write(&path, json).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tempfile::TempDir;

    fn fake_checksum() -> &'static str {
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    }

    #[tokio::test]
    async fn test_write_and_read_text() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_str().unwrap();
        write_extract(base, fake_checksum(), &Extractable::Text("hello document".into())).await.unwrap();
        let result = read_extract(base, fake_checksum()).await;
        assert!(matches!(result, Some(ExtractForQuery::Text(t)) if t == "hello document"));
    }

    #[tokio::test]
    async fn test_write_and_read_scanned_pdf() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_str().unwrap();
        let raw = "garbled ocr text";
        write_extract(base, fake_checksum(), &Extractable::ScannedPdf(Bytes::from("pdfbytes"), raw.into())).await.unwrap();
        let result = read_extract(base, fake_checksum()).await;
        assert!(matches!(result, Some(ExtractForQuery::ScannedPdf(t)) if t == raw));
    }

    #[tokio::test]
    async fn test_write_image_bytes_is_noop() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_str().unwrap();
        write_extract(base, fake_checksum(), &Extractable::ImageBytes(Bytes::from(vec![0u8; 4]))).await.unwrap();
        assert!(read_extract(base, fake_checksum()).await.is_none());
    }

    #[tokio::test]
    async fn test_read_missing_returns_none() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_str().unwrap();
        assert!(read_extract(base, fake_checksum()).await.is_none());
    }

    #[tokio::test]
    async fn test_same_checksum_shared_cache() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_str().unwrap();
        write_extract(base, fake_checksum(), &Extractable::Text("first".into())).await.unwrap();
        write_extract(base, fake_checksum(), &Extractable::Text("second".into())).await.unwrap();
        let result = read_extract(base, fake_checksum()).await;
        assert!(matches!(result, Some(ExtractForQuery::Text(t)) if t == "second"));
    }
}
