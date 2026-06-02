// src/db/refresh_token_ops.rs
use crate::{
    db::{refresh_token_schema, DbClient},
    error::AppError,
    models::RefreshToken,
};
use arrow_array::{Array, Int64Array, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert_refresh_token(&self, record: &RefreshToken) -> Result<(), AppError> {
        let batch = refresh_token_to_batch(record)?;
        let iter = RecordBatchIterator::new(vec![Ok(batch)], refresh_token_schema());
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.refresh_token_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn upsert_refresh_token(&self, record: &RefreshToken) -> Result<(), AppError> {
        let batch = refresh_token_to_batch(record)?;
        let iter = RecordBatchIterator::new(vec![Ok(batch)], refresh_token_schema());
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.refresh_token_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_refresh_token_by_hash(&self, token_hash: &str) -> Result<Option<RefreshToken>, AppError> {
        let escaped = token_hash.replace('\'', "''");
        let stream = self.refresh_token_table.query()
            .only_if(format!("token_hash = '{escaped}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_refresh_tokens(collect_stream(stream).await?)?;
        Ok(records.pop())
    }

    pub async fn list_refresh_tokens_by_family(&self, family_id: Uuid) -> Result<Vec<RefreshToken>, AppError> {
        let fam = family_id.to_string();
        let stream = self.refresh_token_table.query()
            .only_if(format!("family_id = '{fam}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_refresh_tokens(collect_stream(stream).await?)
    }

    /// Revoke every row in a family (theft response / logout). Sets `revoked_at` on each.
    pub async fn revoke_refresh_token_family(&self, family_id: Uuid, now: chrono::DateTime<chrono::Utc>) -> Result<(), AppError> {
        let rows = self.list_refresh_tokens_by_family(family_id).await?;
        for mut row in rows {
            if row.revoked_at.is_none() {
                row.revoked_at = Some(now);
                self.upsert_refresh_token(&row).await?;
            }
        }
        Ok(())
    }
}

fn refresh_token_to_batch(r: &RefreshToken) -> Result<RecordBatch, AppError> {
    let schema = refresh_token_schema();
    let id = r.id.to_string();
    let subject_id = r.subject_id.to_string();
    let client_id = r.client_id.map(|c| c.to_string());
    let family_id = r.family_id.to_string();
    let issued = r.issued_at.to_rfc3339();
    let expires = r.expires_at.to_rfc3339();
    let consumed = r.consumed_at.as_ref().map(|d| d.to_rfc3339());
    let revoked = r.revoked_at.as_ref().map(|d| d.to_rfc3339());
    let last_used = r.last_used_at.as_ref().map(|d| d.to_rfc3339());

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![id.as_str()])),
        Arc::new(StringArray::from(vec![r.token_hash.as_str()])),
        Arc::new(StringArray::from(vec![r.subject_type.as_str()])),
        Arc::new(StringArray::from(vec![subject_id.as_str()])),
        Arc::new(StringArray::from(vec![client_id.as_deref()])),
        Arc::new(StringArray::from(vec![family_id.as_str()])),
        Arc::new(Int64Array::from(vec![r.token_version])),
        Arc::new(StringArray::from(vec![issued.as_str()])),
        Arc::new(StringArray::from(vec![expires.as_str()])),
        Arc::new(StringArray::from(vec![consumed.as_deref()])),
        Arc::new(StringArray::from(vec![revoked.as_deref()])),
        Arc::new(StringArray::from(vec![last_used.as_deref()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_refresh_tokens(batches: Vec<RecordBatch>) -> Result<Vec<RefreshToken>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            out.push(row_to_refresh_token(batch, i)?);
        }
    }
    Ok(out)
}

fn row_to_refresh_token(batch: &RecordBatch, i: usize) -> Result<RefreshToken, AppError> {
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
            .unwrap_or_default()
    };
    let parse_dt = |s: String| s.parse::<chrono::DateTime<chrono::Utc>>()
        .map_err(|e| AppError::Internal(e.to_string()));
    let parse_uuid = |s: String| s.parse::<Uuid>()
        .map_err(|e: uuid::Error| AppError::Internal(e.to_string()));

    Ok(RefreshToken {
        id: parse_uuid(str_col("id"))?,
        token_hash: str_col("token_hash"),
        subject_type: str_col("subject_type"),
        subject_id: parse_uuid(str_col("subject_id"))?,
        client_id: opt_str("client_id").map(parse_uuid).transpose()?,
        family_id: parse_uuid(str_col("family_id"))?,
        token_version: i64_col("token_version"),
        issued_at: parse_dt(str_col("issued_at"))?,
        expires_at: parse_dt(str_col("expires_at"))?,
        consumed_at: opt_str("consumed_at").map(parse_dt).transpose()?,
        revoked_at: opt_str("revoked_at").map(parse_dt).transpose()?,
        last_used_at: opt_str("last_used_at").map(parse_dt).transpose()?,
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

    fn sample(family_id: Uuid, hash: &str) -> RefreshToken {
        let now = Utc::now();
        RefreshToken {
            id: Uuid::new_v4(),
            token_hash: hash.into(),
            subject_type: "fleet_user".into(),
            subject_id: Uuid::new_v4(),
            client_id: None,
            family_id,
            token_version: 0,
            issued_at: now,
            expires_at: now + chrono::Duration::days(14),
            consumed_at: None,
            revoked_at: None,
            last_used_at: None,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_by_hash() {
        let (db, _d) = test_db().await;
        let rt = sample(Uuid::new_v4(), "hash_a");
        db.insert_refresh_token(&rt).await.unwrap();
        let got = db.get_refresh_token_by_hash("hash_a").await.unwrap().unwrap();
        assert_eq!(got.id, rt.id);
        assert_eq!(got.token_version, 0);
        assert!(got.client_id.is_none());
    }

    #[tokio::test]
    async fn test_get_by_hash_missing() {
        let (db, _d) = test_db().await;
        assert!(db.get_refresh_token_by_hash("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_upsert_marks_consumed() {
        let (db, _d) = test_db().await;
        let mut rt = sample(Uuid::new_v4(), "hash_b");
        db.insert_refresh_token(&rt).await.unwrap();
        rt.consumed_at = Some(Utc::now());
        db.upsert_refresh_token(&rt).await.unwrap();
        let got = db.get_refresh_token_by_hash("hash_b").await.unwrap().unwrap();
        assert!(got.consumed_at.is_some());
    }

    #[tokio::test]
    async fn test_revoke_family_revokes_all_rows() {
        let (db, _d) = test_db().await;
        let fam = Uuid::new_v4();
        db.insert_refresh_token(&sample(fam, "hash_c1")).await.unwrap();
        db.insert_refresh_token(&sample(fam, "hash_c2")).await.unwrap();
        db.revoke_refresh_token_family(fam, Utc::now()).await.unwrap();
        let rows = db.list_refresh_tokens_by_family(fam).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.revoked_at.is_some()));
    }
}
