// src/db/dispatcher_ops.rs
use crate::{
    db::{dispatcher_credentials_schema, dispatcher_schema, DbClient},
    error::AppError,
    models::{DispatcherCredentials, DispatcherRecord},
};
use arrow_array::{Array, Int32Array, Int64Array, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert_dispatcher(&self, record: &DispatcherRecord) -> Result<(), AppError> {
        let batch = dispatcher_to_batch(record)?;
        let schema = dispatcher_schema();
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.dispatcher_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_dispatcher_by_id(&self, id: Uuid) -> Result<DispatcherRecord, AppError> {
        let id_str = id.to_string();
        let stream = self.dispatcher_table.query()
            .only_if(format!("id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_dispatchers(collect_stream(stream).await?)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    pub async fn get_dispatcher_by_email(&self, email: &str) -> Result<Option<DispatcherRecord>, AppError> {
        let escaped = email.replace('\'', "''");
        let stream = self.dispatcher_table.query()
            .only_if(format!("email = '{escaped}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let records = batches_to_dispatchers(collect_stream(stream).await?)?;
        Ok(records.into_iter().next())
    }

    pub async fn list_dispatchers(&self) -> Result<Vec<DispatcherRecord>, AppError> {
        let stream = self.dispatcher_table.query()
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_dispatchers(collect_stream(stream).await?)
    }

    pub async fn upsert_dispatcher(&self, record: &DispatcherRecord) -> Result<(), AppError> {
        let batch = dispatcher_to_batch(record)?;
        let schema = dispatcher_schema();
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.dispatcher_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_dispatcher_credentials(&self, id: Uuid) -> Result<Option<DispatcherCredentials>, AppError> {
        let id_str = id.to_string();
        let stream = self.dispatcher_credentials_table.query()
            .only_if(format!("dispatcher_id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let batches = collect_stream(stream).await?;
        let mut records = batches_to_dispatcher_credentials(batches)?;
        Ok(records.pop())
    }

    pub async fn upsert_dispatcher_credentials(&self, record: &DispatcherCredentials) -> Result<(), AppError> {
        let batch = dispatcher_credentials_to_batch(record)?;
        let schema = dispatcher_credentials_schema();
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.dispatcher_credentials_table.merge_insert(&["dispatcher_id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }
}

// --- Batch helpers ---

fn dispatcher_to_batch(record: &DispatcherRecord) -> Result<RecordBatch, AppError> {
    let schema = dispatcher_schema();
    let id_str = record.id.to_string();
    let role_str = record.role.as_str();
    let extra_scopes_str = serde_json::to_string(&record.extra_scopes)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let created_str = record.created_at.to_rfc3339();
    let updated_str = record.updated_at.to_rfc3339();

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![id_str.as_str()])),
        Arc::new(StringArray::from(vec![record.email.as_str()])),
        Arc::new(StringArray::from(vec![record.name.as_str()])),
        Arc::new(StringArray::from(vec![record.status.as_str()])),
        Arc::new(StringArray::from(vec![role_str])),
        Arc::new(StringArray::from(vec![extra_scopes_str.as_str()])),
        Arc::new(StringArray::from(vec![created_str.as_str()])),
        Arc::new(StringArray::from(vec![updated_str.as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn dispatcher_credentials_to_batch(record: &DispatcherCredentials) -> Result<RecordBatch, AppError> {
    let schema = dispatcher_credentials_schema();
    let dispatcher_id_str = record.dispatcher_id.to_string();
    let updated_str = record.updated_at.to_rfc3339();
    let locked_str = record.locked_until.as_ref().map(|dt| dt.to_rfc3339());

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![dispatcher_id_str.as_str()])),
        Arc::new(StringArray::from(vec![record.password_hash.as_str()])),
        Arc::new(Int64Array::from(vec![record.token_version])),
        Arc::new(Int32Array::from(vec![record.failed_attempts])),
        Arc::new(StringArray::from(vec![locked_str.as_deref()])),
        Arc::new(StringArray::from(vec![updated_str.as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_dispatchers(batches: Vec<RecordBatch>) -> Result<Vec<DispatcherRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            out.push(row_to_dispatcher(batch, i)?);
        }
    }
    Ok(out)
}

fn row_to_dispatcher(batch: &RecordBatch, i: usize) -> Result<DispatcherRecord, AppError> {
    let str_col = |name: &str| -> String {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string())
            .unwrap_or_default()
    };

    let role = str_col("role")
        .parse()
        .unwrap_or(crate::models::Role::Dispatcher);
    let extra_scopes_raw = str_col("extra_scopes");
    let extra_scopes = if extra_scopes_raw.is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(&extra_scopes_raw).unwrap_or_default()
    };

    Ok(DispatcherRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        email: str_col("email"),
        name: str_col("name"),
        status: str_col("status").parse().map_err(|e: String| AppError::Internal(e))?,
        role,
        extra_scopes,
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn batches_to_dispatcher_credentials(batches: Vec<RecordBatch>) -> Result<Vec<DispatcherCredentials>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() {
            out.push(row_to_dispatcher_credentials(batch, i)?);
        }
    }
    Ok(out)
}

fn row_to_dispatcher_credentials(batch: &RecordBatch, i: usize) -> Result<DispatcherCredentials, AppError> {
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
    let i32_col = |name: &str| -> i32 {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
            .map(|a| a.value(i))
            .unwrap_or(0)
    };

    let locked_until = opt_str("locked_until")
        .map(|s| s.parse::<chrono::DateTime<chrono::Utc>>())
        .transpose()
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(DispatcherCredentials {
        dispatcher_id: str_col("dispatcher_id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        password_hash: str_col("password_hash"),
        token_version: i64_col("token_version"),
        failed_attempts: i32_col("failed_attempts"),
        locked_until,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
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
    use crate::models::DispatcherStatus;
    use chrono::Utc;
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn sample_dispatcher() -> DispatcherRecord {
        let now = Utc::now();
        DispatcherRecord {
            id: Uuid::new_v4(),
            email: "dispatch@example.com".into(),
            name: "Jane Dispatcher".into(),
            status: DispatcherStatus::Active,
            role: crate::models::Role::Dispatcher,
            extra_scopes: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_dispatcher() {
        let (db, _dir) = test_db().await;
        let d = sample_dispatcher();
        db.insert_dispatcher(&d).await.unwrap();
        let fetched = db.get_dispatcher_by_id(d.id).await.unwrap();
        assert_eq!(fetched.id, d.id);
        assert_eq!(fetched.email, "dispatch@example.com");
        assert_eq!(fetched.status, DispatcherStatus::Active);
    }

    #[tokio::test]
    async fn test_get_dispatcher_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_dispatcher_by_id(Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_get_dispatcher_by_email() {
        let (db, _dir) = test_db().await;
        let d = sample_dispatcher();
        db.insert_dispatcher(&d).await.unwrap();
        let found = db.get_dispatcher_by_email("dispatch@example.com").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, d.id);

        let not_found = db.get_dispatcher_by_email("other@example.com").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_list_dispatchers() {
        let (db, _dir) = test_db().await;
        let d1 = sample_dispatcher();
        let d2 = DispatcherRecord {
            id: Uuid::new_v4(),
            email: "other@example.com".into(),
            name: "Other Dispatcher".into(),
            status: DispatcherStatus::Inactive,
            role: crate::models::Role::FleetManager,
            extra_scopes: vec!["loads:settle".into()],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        db.insert_dispatcher(&d1).await.unwrap();
        db.insert_dispatcher(&d2).await.unwrap();
        let list = db.list_dispatchers().await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_upsert_dispatcher() {
        let (db, _dir) = test_db().await;
        let mut d = sample_dispatcher();
        db.insert_dispatcher(&d).await.unwrap();
        d.name = "Updated Name".into();
        d.updated_at = Utc::now();
        db.upsert_dispatcher(&d).await.unwrap();
        let fetched = db.get_dispatcher_by_id(d.id).await.unwrap();
        assert_eq!(fetched.name, "Updated Name");
    }

    #[tokio::test]
    async fn test_upsert_and_get_dispatcher_credentials() {
        let (db, _dir) = test_db().await;
        let dispatcher_id = Uuid::new_v4();
        let creds = DispatcherCredentials {
            dispatcher_id,
            password_hash: "$2b$12$hashedpassword".into(),
            token_version: 1,
            failed_attempts: 0,
            locked_until: None,
            updated_at: Utc::now(),
        };
        db.upsert_dispatcher_credentials(&creds).await.unwrap();
        let fetched = db.get_dispatcher_credentials(dispatcher_id).await.unwrap().unwrap();
        assert_eq!(fetched.dispatcher_id, dispatcher_id);
        assert_eq!(fetched.password_hash, "$2b$12$hashedpassword");
        assert_eq!(fetched.token_version, 1);
        assert_eq!(fetched.failed_attempts, 0);
        assert!(fetched.locked_until.is_none());
    }

    #[tokio::test]
    async fn test_dispatcher_credentials_update_via_upsert() {
        let (db, _dir) = test_db().await;
        let dispatcher_id = Uuid::new_v4();
        let creds = DispatcherCredentials {
            dispatcher_id,
            password_hash: "$2b$12$original".into(),
            token_version: 1,
            failed_attempts: 0,
            locked_until: None,
            updated_at: Utc::now(),
        };
        db.upsert_dispatcher_credentials(&creds).await.unwrap();

        let updated = DispatcherCredentials {
            password_hash: "$2b$12$updated".into(),
            token_version: 2,
            failed_attempts: 1,
            ..creds
        };
        db.upsert_dispatcher_credentials(&updated).await.unwrap();

        let fetched = db.get_dispatcher_credentials(dispatcher_id).await.unwrap().unwrap();
        assert_eq!(fetched.token_version, 2);
        assert_eq!(fetched.failed_attempts, 1);
        assert_eq!(fetched.password_hash, "$2b$12$updated");
    }
}
