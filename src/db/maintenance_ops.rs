// src/db/maintenance_ops.rs
use crate::{
    db::{maintenance_schema, DbClient},
    error::AppError,
    models::{MaintenanceCategory, MaintenanceListItem, MaintenanceRecord},
};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Float64Array, Int64Array,
    RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray,
};
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

impl DbClient {
    pub async fn insert_maintenance(&self, record: &MaintenanceRecord) -> Result<(), AppError> {
        let batch = maintenance_to_batch(record, self.embed_dim)?;
        let schema = maintenance_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.maintenance_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_maintenance_by_id(&self, id: Uuid) -> Result<MaintenanceRecord, AppError> {
        let id_str = id.to_string();
        let stream = self.maintenance_table.query()
            .only_if(format!("id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_maintenance(collect_stream(stream).await?)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    async fn upsert_maintenance(&self, record: &MaintenanceRecord) -> Result<(), AppError> {
        let batch = maintenance_to_batch(record, self.embed_dim)?;
        let schema = maintenance_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.maintenance_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_maintenance_metadata(
        &self, id: Uuid,
        service_date: Option<String>,
        category: Option<MaintenanceCategory>,
        description: Option<String>,
        cost: Option<f64>,
        odometer: Option<i64>,
        vendor: Option<String>,
        invoice_ref: Option<String>,
        blob_ids: Option<Vec<Uuid>>,
        expense_id: Option<Uuid>,
    ) -> Result<MaintenanceRecord, AppError> {
        let mut record = self.get_maintenance_by_id(id).await?;
        if let Some(v) = service_date { record.service_date = v; }
        if let Some(v) = category { record.category = v; }
        if let Some(v) = description { record.description = v; }
        if let Some(v) = cost { record.cost = Some(v); }
        if let Some(v) = odometer { record.odometer = Some(v); }
        if let Some(v) = vendor { record.vendor = Some(v); }
        if let Some(v) = invoice_ref { record.invoice_ref = Some(v); }
        if let Some(v) = blob_ids { record.blob_ids = v; }
        if let Some(v) = expense_id { record.expense_id = Some(v); }
        record.updated_at = Utc::now();
        self.upsert_maintenance(&record).await?;
        Ok(record)
    }

    pub async fn update_maintenance_embedding(&self, id: Uuid, embedding: Vec<f32>) -> Result<(), AppError> {
        let mut record = self.get_maintenance_by_id(id).await?;
        record.embedding = Some(embedding);
        record.updated_at = Utc::now();
        self.upsert_maintenance(&record).await
    }

    /// Hard delete — a maintenance entry is a correctable log row.
    pub async fn delete_maintenance(&self, id: Uuid) -> Result<(), AppError> {
        let id_str = id.to_string();
        self.maintenance_table
            .delete(&format!("id = '{id_str}'"))
            .await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn list_maintenance(
        &self,
        equipment_type: Option<&str>,
        equipment_id: Option<&str>,
        category: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(usize, Vec<MaintenanceListItem>), AppError> {
        let filter = build_maintenance_filter(equipment_type, equipment_id, category);
        let total = self.maintenance_table.count_rows(filter.clone()).await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut q = self.maintenance_table.query();
        if let Some(f) = filter { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_maintenance(collect_stream(stream).await?)?;
        // Most-recent service first; tie-break on created_at desc for stability.
        records.sort_by(|a, b| {
            b.service_date.cmp(&a.service_date)
                .then(b.created_at.cmp(&a.created_at))
        });
        let items: Vec<MaintenanceListItem> = records.into_iter()
            .skip(offset).take(limit).map(MaintenanceListItem::from).collect();
        Ok((total, items))
    }

    pub async fn any_maintenance_references_blob(&self, blob_id: Uuid) -> Result<bool, AppError> {
        let id_str = blob_id.to_string();
        let count = self.maintenance_table
            .count_rows(Some(format!("blob_ids LIKE '%\"{id_str}\"%'")))
            .await.map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(count > 0)
    }

    pub async fn maintenance_referencing_blob(&self, blob_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        let id_str = blob_id.to_string();
        let stream = self.maintenance_table.query()
            .only_if(format!("blob_ids LIKE '%\"{id_str}\"%'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok(batches_to_maintenance(collect_stream(stream).await?)?
            .into_iter().map(|r| r.id).collect())
    }

    pub async fn create_maintenance_vector_index(&self) -> Result<(), AppError> {
        self.create_ivfpq_index(&self.maintenance_table, "embedding", "maintenance").await
    }
}

// --- Helpers ---

fn maintenance_to_batch(record: &MaintenanceRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = maintenance_schema(embed_dim);
    let id_str = record.id.to_string();
    let equipment_type_str = record.equipment_type.as_str();
    let equipment_id_str = record.equipment_id.to_string();
    let category_str = record.category.as_str();
    let created_str = record.created_at.to_rfc3339();
    let updated_str = record.updated_at.to_rfc3339();
    let blob_ids_json = serde_json::to_string(&record.blob_ids)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let expense_id_str = record.expense_id.map(|u| u.to_string());

    let embedding_col: Arc<dyn arrow_array::Array> = match &record.embedding {
        Some(v) => {
            let floats: Vec<Option<f32>> = v.iter().map(|&f| Some(f)).collect();
            Arc::new(FixedSizeListArray::from_iter_primitive::<
                arrow_array::types::Float32Type, _, _
            >(vec![Some(floats)], embed_dim as i32))
        }
        None => Arc::new(FixedSizeListArray::from_iter_primitive::<
            arrow_array::types::Float32Type, _, _
        >(vec![None::<Vec<Option<f32>>>], embed_dim as i32)),
    };

    RecordBatch::try_new(schema, vec![
        Arc::new(StringArray::from(vec![id_str.as_str()])),
        Arc::new(StringArray::from(vec![equipment_type_str])),
        Arc::new(StringArray::from(vec![equipment_id_str.as_str()])),
        Arc::new(StringArray::from(vec![record.service_date.as_str()])),
        Arc::new(StringArray::from(vec![category_str])),
        Arc::new(StringArray::from(vec![record.description.as_str()])),
        Arc::new(Float64Array::from(vec![record.cost])),
        Arc::new(Int64Array::from(vec![record.odometer])),
        Arc::new(StringArray::from(vec![record.vendor.as_deref()])),
        Arc::new(StringArray::from(vec![record.invoice_ref.as_deref()])),
        embedding_col,
        Arc::new(Int64Array::from(vec![record.owner_id])),
        Arc::new(StringArray::from(vec![created_str.as_str()])),
        Arc::new(StringArray::from(vec![updated_str.as_str()])),
        Arc::new(StringArray::from(vec![blob_ids_json.as_str()])),
        Arc::new(StringArray::from(vec![expense_id_str.as_deref()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_maintenance(batches: Vec<RecordBatch>) -> Result<Vec<MaintenanceRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() { out.push(row_to_maintenance(batch, i)?); }
    }
    Ok(out)
}

fn row_to_maintenance(batch: &RecordBatch, i: usize) -> Result<MaintenanceRecord, AppError> {
    let str_col = |name: &str| -> String {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string()).unwrap_or_default()
    };
    let opt_str = |name: &str| -> Option<String> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i).to_string()) })
    };
    let i64_col = |name: &str| -> i64 {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(i)).unwrap_or(0)
    };
    let opt_i64 = |name: &str| -> Option<i64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i)) })
    };
    let opt_f64 = |name: &str| -> Option<f64> {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
            .and_then(|a| if a.is_null(i) { None } else { Some(a.value(i)) })
    };

    let embedding = batch.column_by_name("embedding")
        .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
        .and_then(|fsl| {
            if fsl.is_null(i) { return None; }
            let values = fsl.value(i);
            values.as_any().downcast_ref::<Float32Array>()
                .map(|fa| (0..fa.len()).map(|j| fa.value(j)).collect::<Vec<f32>>())
        });

    Ok(MaintenanceRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        equipment_type: str_col("equipment_type").parse().map_err(AppError::Internal)?,
        equipment_id: str_col("equipment_id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        service_date: str_col("service_date"),
        category: str_col("category").parse().map_err(AppError::Internal)?,
        description: str_col("description"),
        cost: opt_f64("cost"),
        odometer: opt_i64("odometer"),
        vendor: opt_str("vendor"),
        invoice_ref: opt_str("invoice_ref"),
        blob_ids: serde_json::from_str(&str_col("blob_ids")).unwrap_or_default(),
        expense_id: opt_str("expense_id")
            .map(|s| s.parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string())))
            .transpose()?,
        embedding,
        owner_id: i64_col("owner_id"),
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_maintenance_filter(
    equipment_type: Option<&str>,
    equipment_id: Option<&str>,
    category: Option<&str>,
) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    if let Some(t) = equipment_type {
        clauses.push(format!("equipment_type = '{}'", t.replace('\'', "''")));
    }
    if let Some(id) = equipment_id {
        clauses.push(format!("equipment_id = '{}'", id.replace('\'', "''")));
    }
    if let Some(c) = category {
        clauses.push(format!("category = '{}'", c.replace('\'', "''")));
    }
    if clauses.is_empty() { None } else { Some(clauses.join(" AND ")) }
}

async fn collect_stream(
    stream: impl futures::TryStream<Ok = RecordBatch, Error = impl std::error::Error + Send + Sync + 'static> + Send,
) -> Result<Vec<RecordBatch>, AppError> {
    stream.try_collect::<Vec<_>>().await.map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EquipmentType;
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn sample(equipment_id: Uuid) -> MaintenanceRecord {
        let now = Utc::now();
        MaintenanceRecord {
            id: Uuid::new_v4(),
            equipment_type: EquipmentType::Truck,
            equipment_id,
            service_date: "2026-06-01".into(),
            category: MaintenanceCategory::Repair,
            description: "replaced alternator".into(),
            cost: Some(412.50),
            odometer: Some(184000),
            vendor: Some("Acme Diesel".into()),
            invoice_ref: Some("INV-9931".into()),
            blob_ids: vec![],
            expense_id: None,
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let (db, _dir) = test_db().await;
        let m = sample(Uuid::new_v4());
        db.insert_maintenance(&m).await.unwrap();
        let got = db.get_maintenance_by_id(m.id).await.unwrap();
        assert_eq!(got.id, m.id);
        assert_eq!(got.description, "replaced alternator");
        assert_eq!(got.cost, Some(412.50));
        assert_eq!(got.odometer, Some(184000));
        assert_eq!(got.category, MaintenanceCategory::Repair);
        assert_eq!(got.equipment_type, EquipmentType::Truck);
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_maintenance_by_id(Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_update_metadata() {
        let (db, _dir) = test_db().await;
        let m = sample(Uuid::new_v4());
        db.insert_maintenance(&m).await.unwrap();
        let updated = db.update_maintenance_metadata(
            m.id,
            None,
            Some(MaintenanceCategory::Brakes),
            Some("front brake pads".into()),
            Some(220.0),
            None, None, None, None, None,
        ).await.unwrap();
        assert_eq!(updated.category, MaintenanceCategory::Brakes);
        assert_eq!(updated.description, "front brake pads");
        assert_eq!(updated.cost, Some(220.0));
        assert_eq!(updated.odometer, Some(184000));
    }

    #[tokio::test]
    async fn test_hard_delete() {
        let (db, _dir) = test_db().await;
        let m = sample(Uuid::new_v4());
        db.insert_maintenance(&m).await.unwrap();
        db.delete_maintenance(m.id).await.unwrap();
        assert!(matches!(db.get_maintenance_by_id(m.id).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_list_filtered_by_equipment() {
        let (db, _dir) = test_db().await;
        let eq_a = Uuid::new_v4();
        let eq_b = Uuid::new_v4();
        let mut m1 = sample(eq_a);
        m1.service_date = "2026-05-01".into();
        let mut m2 = sample(eq_a);
        m2.service_date = "2026-06-15".into();
        let m3 = sample(eq_b);
        db.insert_maintenance(&m1).await.unwrap();
        db.insert_maintenance(&m2).await.unwrap();
        db.insert_maintenance(&m3).await.unwrap();

        let (total, items) = db.list_maintenance(
            Some("truck"), Some(&eq_a.to_string()), None, 50, 0,
        ).await.unwrap();
        assert_eq!(total, 2);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].service_date, "2026-06-15");
        assert_eq!(items[1].service_date, "2026-05-01");

        let (total_b, _) = db.list_maintenance(
            Some("truck"), Some(&eq_b.to_string()), None, 50, 0,
        ).await.unwrap();
        assert_eq!(total_b, 1);
    }

    #[tokio::test]
    async fn test_list_filtered_by_category() {
        let (db, _dir) = test_db().await;
        let eq = Uuid::new_v4();
        let mut tire = sample(eq);
        tire.category = MaintenanceCategory::Tire;
        let repair = sample(eq);
        db.insert_maintenance(&tire).await.unwrap();
        db.insert_maintenance(&repair).await.unwrap();

        let (total, items) = db.list_maintenance(None, None, Some("tire"), 50, 0).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].category, MaintenanceCategory::Tire);
    }
}
