// src/db/authorization_code_ops.rs
use crate::{
    db::{authorization_code_schema, DbClient},
    error::AppError,
    models::AuthorizationCode,
};
use arrow_array::{Array, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;

impl DbClient {
    pub async fn insert_authorization_code(&self, c: &AuthorizationCode) -> Result<(), AppError> {
        let batch = code_to_batch(c)?;
        let iter = RecordBatchIterator::new(vec![Ok(batch)], authorization_code_schema());
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.authorization_code_table.add(reader).execute().await
            .map(|_| ()).map_err(|e| AppError::Internal(e.to_string()))
    }

    /// Validate + consume in one call. Returns the code row on success, or
    /// `None` if missing / expired / already consumed.
    pub async fn consume_authorization_code(
        &self, code_hash: &str, now: DateTime<Utc>,
    ) -> Result<Option<AuthorizationCode>, AppError> {
        let escaped = code_hash.replace('\'', "''");
        let stream = self.authorization_code_table.query()
            .only_if(format!("code_hash = '{escaped}'"))
            .execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let mut rows = batches_to_codes(collect_stream(stream).await?)?;
        let row = match rows.pop() {
            Some(r) => r,
            None => return Ok(None),
        };
        if row.consumed_at.is_some() || row.expires_at <= now {
            return Ok(None);
        }
        let mut consumed = row.clone();
        consumed.consumed_at = Some(now);
        let batch = code_to_batch(&consumed)?;
        let iter = RecordBatchIterator::new(vec![Ok(batch)], authorization_code_schema());
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.authorization_code_table.merge_insert(&["code_hash"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await.map(|_| ()).map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(Some(row))
    }
}

fn code_to_batch(c: &AuthorizationCode) -> Result<RecordBatch, AppError> {
    let client_id = c.client_id.to_string();
    let subject_id = c.subject_id.to_string();
    let created = c.created_at.to_rfc3339();
    let expires = c.expires_at.to_rfc3339();
    let consumed = c.consumed_at.as_ref().map(|d| d.to_rfc3339());
    RecordBatch::try_new(authorization_code_schema(), vec![
        Arc::new(StringArray::from(vec![c.code_hash.as_str()])),
        Arc::new(StringArray::from(vec![client_id.as_str()])),
        Arc::new(StringArray::from(vec![c.redirect_uri.as_str()])),
        Arc::new(StringArray::from(vec![c.code_challenge.as_str()])),
        Arc::new(StringArray::from(vec![c.subject_type.as_str()])),
        Arc::new(StringArray::from(vec![subject_id.as_str()])),
        Arc::new(StringArray::from(vec![c.resource.as_str()])),
        Arc::new(StringArray::from(vec![c.scope.as_deref()])),
        Arc::new(StringArray::from(vec![created.as_str()])),
        Arc::new(StringArray::from(vec![expires.as_str()])),
        Arc::new(StringArray::from(vec![consumed.as_deref()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_codes(batches: Vec<RecordBatch>) -> Result<Vec<AuthorizationCode>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            let str_col = |name: &str| batch.column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| a.value(i).to_string()).unwrap_or_default();
            let opt_str = |name: &str| batch.column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i).to_string()) });
            let parse_dt = |s: String| s.parse::<DateTime<Utc>>().map_err(|e| AppError::Internal(e.to_string()));
            out.push(AuthorizationCode {
                code_hash: str_col("code_hash"),
                client_id: str_col("client_id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
                redirect_uri: str_col("redirect_uri"),
                code_challenge: str_col("code_challenge"),
                subject_type: str_col("subject_type"),
                subject_id: str_col("subject_id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
                resource: str_col("resource"),
                scope: opt_str("scope"),
                created_at: parse_dt(str_col("created_at"))?,
                expires_at: parse_dt(str_col("expires_at"))?,
                consumed_at: opt_str("consumed_at").map(parse_dt).transpose()?,
            });
        }
    }
    Ok(out)
}

async fn collect_stream(
    stream: impl futures::TryStream<Ok = RecordBatch, Error = impl std::error::Error + Send + Sync + 'static> + Send,
) -> Result<Vec<RecordBatch>, AppError> {
    stream.try_collect::<Vec<_>>().await.map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use tempfile::TempDir;

    fn sample(hash: &str, expires: DateTime<Utc>) -> AuthorizationCode {
        AuthorizationCode {
            code_hash: hash.into(),
            client_id: Uuid::new_v4(),
            redirect_uri: "http://127.0.0.1/cb".into(),
            code_challenge: "chal".into(),
            subject_type: "dispatcher".into(),
            subject_id: Uuid::new_v4(),
            resource: "https://x/dispatch/mcp".into(),
            scope: None,
            created_at: Utc::now(),
            expires_at: expires,
            consumed_at: None,
        }
    }

    #[tokio::test]
    async fn test_consume_once() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        db.insert_authorization_code(&sample("h1", Utc::now() + chrono::Duration::minutes(5))).await.unwrap();
        assert!(db.consume_authorization_code("h1", Utc::now()).await.unwrap().is_some());
        assert!(db.consume_authorization_code("h1", Utc::now()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_consume_expired() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        db.insert_authorization_code(&sample("h2", Utc::now() - chrono::Duration::minutes(1))).await.unwrap();
        assert!(db.consume_authorization_code("h2", Utc::now()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_consume_missing() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        assert!(db.consume_authorization_code("nope", Utc::now()).await.unwrap().is_none());
    }
}
