// src/db/driver_credentials_ops.rs
use crate::{
    db::{driver_credentials_schema, driver_passkey_credentials_schema, DbClient},
    error::AppError,
    models::{DriverCredentials, DriverPasskeyCredential},
};
use arrow_array::{Array, Int64Array, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn upsert_driver_credentials(&self, record: &DriverCredentials) -> Result<(), AppError> {
        let batch = driver_credentials_to_batch(record)?;
        let schema = driver_credentials_schema();
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.driver_credentials_table.merge_insert(&["driver_id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await.map(|_| ()).map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_driver_credentials(&self, driver_id: Uuid) -> Result<Option<DriverCredentials>, AppError> {
        let id_str = driver_id.to_string();
        let stream = self.driver_credentials_table.query()
            .only_if(format!("driver_id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let batches = collect_stream(stream).await?;
        let mut records = batches_to_driver_credentials(batches)?;
        Ok(records.pop())
    }

    pub async fn upsert_passkey_credential(&self, record: &DriverPasskeyCredential) -> Result<(), AppError> {
        let batch = passkey_credential_to_batch(record)?;
        let schema = driver_passkey_credentials_schema();
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.driver_passkey_credentials_table.merge_insert(&["credential_id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await.map(|_| ()).map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_passkey_credentials_for_driver(&self, driver_id: Uuid) -> Result<Vec<DriverPasskeyCredential>, AppError> {
        let id_str = driver_id.to_string();
        let stream = self.driver_passkey_credentials_table.query()
            .only_if(format!("driver_id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let batches = collect_stream(stream).await?;
        batches_to_passkey_credentials(batches)
    }

    pub async fn get_passkey_credential(&self, credential_id: &str) -> Result<Option<DriverPasskeyCredential>, AppError> {
        let escaped = credential_id.replace('\'', "''");
        let stream = self.driver_passkey_credentials_table.query()
            .only_if(format!("credential_id = '{escaped}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let batches = collect_stream(stream).await?;
        let mut records = batches_to_passkey_credentials(batches)?;
        Ok(records.pop())
    }
}

// --- Batch helpers ---

fn driver_credentials_to_batch(record: &DriverCredentials) -> Result<RecordBatch, AppError> {
    let schema = driver_credentials_schema();
    let driver_id_str = record.driver_id.to_string();
    let updated_str = record.updated_at.to_rfc3339();
    let locked_str = record.locked_until.as_ref().map(|dt| dt.to_rfc3339());

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![driver_id_str.as_str()])),
        Arc::new(StringArray::from(vec![record.pin_hash.as_deref()])),
        Arc::new(Int64Array::from(vec![record.token_version])),
        Arc::new(Int64Array::from(vec![record.failed_pin_attempts])),
        Arc::new(StringArray::from(vec![locked_str.as_deref()])),
        Arc::new(StringArray::from(vec![updated_str.as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn passkey_credential_to_batch(record: &DriverPasskeyCredential) -> Result<RecordBatch, AppError> {
    let schema = driver_passkey_credentials_schema();
    let driver_id_str = record.driver_id.to_string();
    let created_str = record.created_at.to_rfc3339();

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![record.credential_id.as_str()])),
        Arc::new(StringArray::from(vec![driver_id_str.as_str()])),
        Arc::new(StringArray::from(vec![record.public_key.as_str()])),
        Arc::new(Int64Array::from(vec![record.counter])),
        Arc::new(StringArray::from(vec![record.transports.as_str()])),
        Arc::new(StringArray::from(vec![created_str.as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_driver_credentials(batches: Vec<RecordBatch>) -> Result<Vec<DriverCredentials>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            out.push(row_to_driver_credentials(batch, i)?);
        }
    }
    Ok(out)
}

fn row_to_driver_credentials(batch: &RecordBatch, i: usize) -> Result<DriverCredentials, AppError> {
    let str_col = |name: &str| -> String {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string())
            .unwrap_or_default()
    };
    let opt_str = |name: &str| -> Option<String> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i).to_string()) })
    };
    let i64_col = |name: &str| -> i64 {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(i))
            .unwrap_or(0)
    };

    let locked_until = opt_str("locked_until")
        .map(|s| s.parse::<chrono::DateTime<chrono::Utc>>())
        .transpose()
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(DriverCredentials {
        driver_id: str_col("driver_id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        pin_hash: opt_str("pin_hash"),
        token_version: i64_col("token_version"),
        failed_pin_attempts: i64_col("failed_pin_attempts"),
        locked_until,
        updated_at: str_col("updated_at").parse().map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn batches_to_passkey_credentials(batches: Vec<RecordBatch>) -> Result<Vec<DriverPasskeyCredential>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            out.push(row_to_passkey_credential(batch, i)?);
        }
    }
    Ok(out)
}

fn row_to_passkey_credential(batch: &RecordBatch, i: usize) -> Result<DriverPasskeyCredential, AppError> {
    let str_col = |name: &str| -> String {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string())
            .unwrap_or_default()
    };
    let i64_col = |name: &str| -> i64 {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(i))
            .unwrap_or(0)
    };

    Ok(DriverPasskeyCredential {
        credential_id: str_col("credential_id"),
        driver_id: str_col("driver_id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        public_key: str_col("public_key"),
        counter: i64_col("counter"),
        transports: str_col("transports"),
        created_at: str_col("created_at").parse().map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

async fn collect_stream(
    stream: impl futures::TryStream<Ok = RecordBatch, Error = impl std::error::Error + Send + Sync + 'static> + Send,
) -> Result<Vec<RecordBatch>, AppError> {
    stream.try_collect::<Vec<_>>().await.map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn sample_credentials(driver_id: Uuid) -> DriverCredentials {
        DriverCredentials {
            driver_id,
            pin_hash: Some("$2b$10$hashedpin".into()),
            token_version: 1,
            failed_pin_attempts: 0,
            locked_until: None,
            updated_at: Utc::now(),
        }
    }

    fn sample_passkey(credential_id: &str, driver_id: Uuid) -> DriverPasskeyCredential {
        DriverPasskeyCredential {
            credential_id: credential_id.to_string(),
            driver_id,
            public_key: "base64encodedkey==".into(),
            counter: 0,
            transports: r#"["internal"]"#.into(),
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_upsert_and_get_driver_credentials() {
        let (db, _dir) = test_db().await;
        let driver_id = Uuid::new_v4();
        let creds = sample_credentials(driver_id);
        db.upsert_driver_credentials(&creds).await.unwrap();
        let fetched = db.get_driver_credentials(driver_id).await.unwrap().unwrap();
        assert_eq!(fetched.driver_id, driver_id);
        assert_eq!(fetched.pin_hash, creds.pin_hash);
        assert_eq!(fetched.token_version, 1);
        assert_eq!(fetched.failed_pin_attempts, 0);
        assert!(fetched.locked_until.is_none());
    }

    #[tokio::test]
    async fn test_driver_credentials_update_via_upsert() {
        let (db, _dir) = test_db().await;
        let driver_id = Uuid::new_v4();
        let creds = sample_credentials(driver_id);
        db.upsert_driver_credentials(&creds).await.unwrap();

        let updated = DriverCredentials {
            token_version: 42,
            failed_pin_attempts: 3,
            ..creds
        };
        db.upsert_driver_credentials(&updated).await.unwrap();

        let fetched = db.get_driver_credentials(driver_id).await.unwrap().unwrap();
        assert_eq!(fetched.token_version, 42);
        assert_eq!(fetched.failed_pin_attempts, 3);
    }

    #[tokio::test]
    async fn test_upsert_and_get_passkey_credential() {
        let (db, _dir) = test_db().await;
        let driver_id = Uuid::new_v4();
        let passkey = sample_passkey("cred-abc123", driver_id);
        db.upsert_passkey_credential(&passkey).await.unwrap();

        let fetched = db.get_passkey_credential("cred-abc123").await.unwrap().unwrap();
        assert_eq!(fetched.credential_id, "cred-abc123");
        assert_eq!(fetched.driver_id, driver_id);
        assert_eq!(fetched.public_key, "base64encodedkey==");
        assert_eq!(fetched.counter, 0);
        assert_eq!(fetched.transports, r#"["internal"]"#);
    }

    #[tokio::test]
    async fn test_get_passkey_credentials_for_driver() {
        let (db, _dir) = test_db().await;
        let driver_a = Uuid::new_v4();
        let driver_b = Uuid::new_v4();

        db.upsert_passkey_credential(&sample_passkey("cred-1", driver_a)).await.unwrap();
        db.upsert_passkey_credential(&sample_passkey("cred-2", driver_a)).await.unwrap();
        db.upsert_passkey_credential(&sample_passkey("cred-3", driver_b)).await.unwrap();

        let a_passkeys = db.get_passkey_credentials_for_driver(driver_a).await.unwrap();
        assert_eq!(a_passkeys.len(), 2);
        let ids: Vec<&str> = a_passkeys.iter().map(|p| p.credential_id.as_str()).collect();
        assert!(ids.contains(&"cred-1"));
        assert!(ids.contains(&"cred-2"));

        let b_passkeys = db.get_passkey_credentials_for_driver(driver_b).await.unwrap();
        assert_eq!(b_passkeys.len(), 1);
        assert_eq!(b_passkeys[0].credential_id, "cred-3");
    }
}
