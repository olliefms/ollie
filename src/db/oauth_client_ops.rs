// src/db/oauth_client_ops.rs
use crate::{
    db::{oauth_client_schema, DbClient},
    error::AppError,
    models::OAuthClient,
};
use arrow_array::{Array, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert_oauth_client(&self, c: &OAuthClient) -> Result<(), AppError> {
        let batch = client_to_batch(c)?;
        let iter = RecordBatchIterator::new(vec![Ok(batch)], oauth_client_schema());
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.oauth_client_table.add(reader).execute().await
            .map(|_| ()).map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_oauth_client(&self, id: Uuid) -> Result<Option<OAuthClient>, AppError> {
        let id = id.to_string();
        let stream = self.oauth_client_table.query()
            .only_if(format!("id = '{id}'"))
            .execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let mut v = batches_to_clients(collect_stream(stream).await?)?;
        Ok(v.pop())
    }
}

fn client_to_batch(c: &OAuthClient) -> Result<RecordBatch, AppError> {
    let id = c.id.to_string();
    let created = c.created_at.to_rfc3339();
    let uris = serde_json::to_string(&c.redirect_uris).map_err(|e| AppError::Internal(e.to_string()))?;
    RecordBatch::try_new(oauth_client_schema(), vec![
        Arc::new(StringArray::from(vec![id.as_str()])),
        Arc::new(StringArray::from(vec![c.client_name.as_deref()])),
        Arc::new(StringArray::from(vec![uris.as_str()])),
        Arc::new(StringArray::from(vec![created.as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_clients(batches: Vec<RecordBatch>) -> Result<Vec<OAuthClient>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            let str_col = |name: &str| batch.column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| a.value(i).to_string()).unwrap_or_default();
            let opt_str = |name: &str| batch.column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i).to_string()) });
            let uris: Vec<String> = serde_json::from_str(&str_col("redirect_uris"))
                .map_err(|e| AppError::Internal(e.to_string()))?;
            out.push(OAuthClient {
                id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
                client_name: opt_str("client_name"),
                redirect_uris: uris,
                created_at: str_col("created_at").parse().map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
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
    use chrono::Utc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_insert_and_get_client() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        let c = OAuthClient {
            id: Uuid::new_v4(),
            client_name: Some("Claude".into()),
            redirect_uris: vec!["http://127.0.0.1:33418/callback".into()],
            created_at: Utc::now(),
        };
        db.insert_oauth_client(&c).await.unwrap();
        let got = db.get_oauth_client(c.id).await.unwrap().unwrap();
        assert_eq!(got.redirect_uris, c.redirect_uris);
        assert_eq!(got.client_name.as_deref(), Some("Claude"));
    }

    #[tokio::test]
    async fn test_get_missing_client() {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        assert!(db.get_oauth_client(Uuid::new_v4()).await.unwrap().is_none());
    }
}
