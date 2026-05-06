pub mod ops;

use crate::error::AppError;
use arrow_array::{
    FixedSizeListArray, Int64Array, RecordBatch, RecordBatchIterator, RecordBatchReader,
    StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use lancedb::Table;
use std::sync::Arc;

pub struct DbClient {
    pub table: Table,
    pub embed_dim: usize,
}

impl DbClient {
    pub async fn new(path: &str, embed_dim: usize) -> Result<Self, AppError> {
        let conn = lancedb::connect(path)
            .execute()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let table = match conn.open_table("blobs").execute().await {
            Ok(t) => t,
            Err(_) => {
                let schema = blob_schema(embed_dim);
                let nulls: Vec<Option<Vec<Option<f32>>>> = vec![];
                let batch = RecordBatch::try_new(
                    schema.clone(),
                    vec![
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(Int64Array::from(Vec::<i64>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(Int64Array::from(Vec::<i64>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(
                            FixedSizeListArray::from_iter_primitive::<
                                arrow_array::types::Float32Type,
                                _,
                                _,
                            >(nulls, embed_dim as i32),
                        ),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                        Arc::new(StringArray::from(Vec::<Option<&str>>::new())),
                    ],
                )
                .map_err(|e| AppError::Internal(e.to_string()))?;

                let iter = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
                let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
                conn.create_table("blobs", reader)
                    .execute()
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?
            }
        };

        Ok(Self { table, embed_dim })
    }
}

pub fn blob_schema(embed_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("owner_id", DataType::Int64, false),
        Field::new("checksum", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("mime_type", DataType::Utf8, false),
        Field::new("size", DataType::Int64, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("error", DataType::Utf8, true),
        Field::new("summary", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                embed_dim as i32,
            ),
            true,
        ),
        Field::new("created_at", DataType::Utf8, false),
        Field::new("updated_at", DataType::Utf8, false),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_db_client_creates_table() {
        let dir = TempDir::new().unwrap();
        let client = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        let count = client.table.count_rows(None).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_schema_has_fixed_size_embedding() {
        let schema = blob_schema(768);
        let field = schema.field_with_name("embedding").unwrap();
        assert!(matches!(field.data_type(), DataType::FixedSizeList(_, 768)));
    }
}
