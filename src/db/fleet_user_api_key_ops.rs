// src/db/fleet_user_api_key_ops.rs
use crate::{
    db::{fleet_user_api_key_schema, DbClient},
    error::AppError,
    models::FleetUserApiKey,
};
use arrow_array::{Array, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray};
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert_fleet_user_api_key(&self, record: &FleetUserApiKey) -> Result<(), AppError> {
        let batch = api_key_to_batch(record)?;
        let schema = fleet_user_api_key_schema();
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.fleet_user_api_key_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn upsert_fleet_user_api_key(&self, record: &FleetUserApiKey) -> Result<(), AppError> {
        let batch = api_key_to_batch(record)?;
        let schema = fleet_user_api_key_schema();
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.fleet_user_api_key_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_fleet_user_api_key_by_hash(&self, key_hash: &str) -> Result<Option<FleetUserApiKey>, AppError> {
        let escaped = key_hash.replace('\'', "''");
        let stream = self.fleet_user_api_key_table.query()
            .only_if(format!("key_hash = '{escaped}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_api_keys(collect_stream(stream).await?)?;
        Ok(records.pop())
    }

    pub async fn get_fleet_user_api_key_by_id(&self, id: Uuid, fleet_user_id: Uuid) -> Result<Option<FleetUserApiKey>, AppError> {
        let id_str = id.to_string();
        let fleet_user_id_str = fleet_user_id.to_string();
        let stream = self.fleet_user_api_key_table.query()
            .only_if(format!("id = '{id_str}' AND fleet_user_id = '{fleet_user_id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_api_keys(collect_stream(stream).await?)?;
        Ok(records.pop())
    }

    pub async fn list_active_fleet_user_api_keys(&self, fleet_user_id: Uuid) -> Result<Vec<FleetUserApiKey>, AppError> {
        let id_str = fleet_user_id.to_string();
        let stream = self.fleet_user_api_key_table.query()
            .only_if(format!("fleet_user_id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let records = batches_to_api_keys(collect_stream(stream).await?)?;
        Ok(records.into_iter().filter(|k| k.revoked_at.is_none()).collect())
    }

    pub async fn count_active_fleet_user_api_keys(&self, fleet_user_id: Uuid) -> Result<usize, AppError> {
        let keys = self.list_active_fleet_user_api_keys(fleet_user_id).await?;
        let now = Utc::now();
        Ok(keys.iter().filter(|k| k.expires_at > now).count())
    }
}

fn api_key_to_batch(record: &FleetUserApiKey) -> Result<RecordBatch, AppError> {
    let schema = fleet_user_api_key_schema();
    let id_str = record.id.to_string();
    let fleet_user_id_str = record.fleet_user_id.to_string();
    let created_str = record.created_at.to_rfc3339();
    let expires_str = record.expires_at.to_rfc3339();
    let revoked_str = record.revoked_at.as_ref().map(|dt| dt.to_rfc3339());
    let last_used_str = record.last_used_at.as_ref().map(|dt| dt.to_rfc3339());

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![id_str.as_str()])),
        Arc::new(StringArray::from(vec![fleet_user_id_str.as_str()])),
        Arc::new(StringArray::from(vec![record.label.as_str()])),
        Arc::new(StringArray::from(vec![record.key_hash.as_str()])),
        Arc::new(StringArray::from(vec![record.key_prefix.as_str()])),
        Arc::new(StringArray::from(vec![created_str.as_str()])),
        Arc::new(StringArray::from(vec![expires_str.as_str()])),
        Arc::new(StringArray::from(vec![revoked_str.as_deref()])),
        Arc::new(StringArray::from(vec![last_used_str.as_deref()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_api_keys(batches: Vec<RecordBatch>) -> Result<Vec<FleetUserApiKey>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            out.push(row_to_api_key(batch, i)?);
        }
    }
    Ok(out)
}

fn row_to_api_key(batch: &RecordBatch, i: usize) -> Result<FleetUserApiKey, AppError> {
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

    Ok(FleetUserApiKey {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        fleet_user_id: str_col("fleet_user_id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        label: str_col("label"),
        key_hash: str_col("key_hash"),
        key_prefix: str_col("key_prefix"),
        created_at: str_col("created_at").parse().map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        expires_at: str_col("expires_at").parse().map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        revoked_at: opt_str("revoked_at")
            .map(|s| s.parse::<chrono::DateTime<chrono::Utc>>())
            .transpose()
            .map_err(|e| AppError::Internal(e.to_string()))?,
        last_used_at: opt_str("last_used_at")
            .map(|s| s.parse::<chrono::DateTime<chrono::Utc>>())
            .transpose()
            .map_err(|e| AppError::Internal(e.to_string()))?,
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

    fn sample_key(fleet_user_id: Uuid) -> FleetUserApiKey {
        let now = Utc::now();
        FleetUserApiKey {
            id: Uuid::new_v4(),
            fleet_user_id,
            label: "Test Key".into(),
            key_hash: "abc123hash".into(),
            key_prefix: "olld_a1b2c3".into(),
            created_at: now,
            expires_at: now + chrono::Duration::days(365),
            revoked_at: None,
            last_used_at: None,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_by_hash() {
        let (db, _dir) = test_db().await;
        let fleet_user_id = Uuid::new_v4();
        let key = sample_key(fleet_user_id);
        let hash = key.key_hash.clone();
        db.insert_fleet_user_api_key(&key).await.unwrap();
        let found = db.get_fleet_user_api_key_by_hash(&hash).await.unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.id, key.id);
        assert_eq!(found.label, "Test Key");
    }

    #[tokio::test]
    async fn test_get_by_hash_not_found() {
        let (db, _dir) = test_db().await;
        let result = db.get_fleet_user_api_key_by_hash("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_list_active_excludes_revoked() {
        let (db, _dir) = test_db().await;
        let fleet_user_id = Uuid::new_v4();
        let active = sample_key(fleet_user_id);
        let mut revoked = sample_key(fleet_user_id);
        revoked.id = Uuid::new_v4();
        revoked.key_hash = "other_hash".into();
        revoked.revoked_at = Some(Utc::now());
        db.insert_fleet_user_api_key(&active).await.unwrap();
        db.insert_fleet_user_api_key(&revoked).await.unwrap();
        let list = db.list_active_fleet_user_api_keys(fleet_user_id).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, active.id);
    }

    #[tokio::test]
    async fn test_list_active_only_own_keys() {
        let (db, _dir) = test_db().await;
        let d1 = Uuid::new_v4();
        let d2 = Uuid::new_v4();
        let k1 = sample_key(d1);
        let mut k2 = sample_key(d2);
        k2.key_hash = "other_hash2".into();
        db.insert_fleet_user_api_key(&k1).await.unwrap();
        db.insert_fleet_user_api_key(&k2).await.unwrap();
        let list = db.list_active_fleet_user_api_keys(d1).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].fleet_user_id, d1);
    }

    #[tokio::test]
    async fn test_upsert_revokes_key() {
        let (db, _dir) = test_db().await;
        let fleet_user_id = Uuid::new_v4();
        let key = sample_key(fleet_user_id);
        db.insert_fleet_user_api_key(&key).await.unwrap();
        let mut revoked = key.clone();
        revoked.revoked_at = Some(Utc::now());
        db.upsert_fleet_user_api_key(&revoked).await.unwrap();
        let list = db.list_active_fleet_user_api_keys(fleet_user_id).await.unwrap();
        assert_eq!(list.len(), 0);
    }

    #[tokio::test]
    async fn test_get_by_id_ownership() {
        let (db, _dir) = test_db().await;
        let d1 = Uuid::new_v4();
        let d2 = Uuid::new_v4();
        let key = sample_key(d1);
        db.insert_fleet_user_api_key(&key).await.unwrap();
        let found = db.get_fleet_user_api_key_by_id(key.id, d1).await.unwrap();
        assert!(found.is_some());
        let not_found = db.get_fleet_user_api_key_by_id(key.id, d2).await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_count_active_excludes_expired_and_revoked() {
        let (db, _dir) = test_db().await;
        let fleet_user_id = Uuid::new_v4();
        let valid = sample_key(fleet_user_id);
        let mut expired = sample_key(fleet_user_id);
        expired.id = Uuid::new_v4();
        expired.key_hash = "hash_expired".into();
        expired.expires_at = Utc::now() - chrono::Duration::days(1);
        let mut revoked = sample_key(fleet_user_id);
        revoked.id = Uuid::new_v4();
        revoked.key_hash = "hash_revoked".into();
        revoked.revoked_at = Some(Utc::now());
        db.insert_fleet_user_api_key(&valid).await.unwrap();
        db.insert_fleet_user_api_key(&expired).await.unwrap();
        db.insert_fleet_user_api_key(&revoked).await.unwrap();
        let count = db.count_active_fleet_user_api_keys(fleet_user_id).await.unwrap();
        assert_eq!(count, 1);
    }
}
