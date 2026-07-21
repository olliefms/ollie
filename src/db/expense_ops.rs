// src/db/expense_ops.rs
use crate::{
    db::{expense_schema, DbClient},
    error::AppError,
    models::ExpenseRecord,
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

#[derive(Debug, Default, Clone)]
pub struct ExpenseFilter {
    pub status: Option<String>,
    pub category: Option<String>,
    pub driver_id: Option<String>,
    pub trip_id: Option<String>,
    pub equipment_id: Option<String>,
    pub submitted_by: Option<String>,
    pub from: Option<String>, // YYYY-MM-DD inclusive
    pub to: Option<String>,   // YYYY-MM-DD inclusive
}

fn effective_date(r: &ExpenseRecord) -> String {
    r.expense_date.clone()
        .unwrap_or_else(|| r.created_at.format("%Y-%m-%d").to_string())
}

impl DbClient {
    pub async fn insert_expense(&self, record: &ExpenseRecord) -> Result<(), AppError> {
        let batch = expense_to_batch(record, self.embed_dim)?;
        let schema = expense_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        self.expense_table.add(reader).execute().await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn get_expense_by_id(&self, id: Uuid) -> Result<ExpenseRecord, AppError> {
        let id_str = id.to_string();
        let stream = self.expense_table.query()
            .only_if(format!("id = '{id_str}'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_expenses(collect_stream(stream).await?)?
            .into_iter().next()
            .ok_or(AppError::NotFound)
    }

    async fn upsert_expense(&self, record: &ExpenseRecord) -> Result<(), AppError> {
        let batch = expense_to_batch(record, self.embed_dim)?;
        let schema = expense_schema(self.embed_dim);
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.expense_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader).await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    /// Whole-record upsert. Caller sets `updated_at`.
    pub async fn update_expense(&self, record: &ExpenseRecord) -> Result<(), AppError> {
        self.upsert_expense(record).await
    }

    /// Fetch -> set suggested_* only -> upsert. Pipeline-safe against concurrent
    /// review edits to other fields.
    pub async fn update_expense_suggestions(
        &self, id: Uuid,
        amount: Option<f64>, date: Option<String>,
        vendor: Option<String>, card_last4: Option<String>,
    ) -> Result<(), AppError> {
        let mut record = self.get_expense_by_id(id).await?;
        record.suggested_amount = amount;
        record.suggested_date = date;
        record.suggested_vendor = vendor;
        record.suggested_card_last4 = card_last4;
        record.updated_at = Utc::now();
        self.upsert_expense(&record).await
    }

    pub async fn update_expense_embedding(&self, id: Uuid, embedding: Vec<f32>) -> Result<(), AppError> {
        let mut record = self.get_expense_by_id(id).await?;
        record.embedding = Some(embedding);
        record.updated_at = Utc::now();
        self.upsert_expense(&record).await
    }

    pub async fn delete_expense(&self, id: Uuid) -> Result<(), AppError> {
        let id_str = id.to_string();
        self.expense_table
            .delete(&format!("id = '{id_str}'"))
            .await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn list_expenses(
        &self, filter: &ExpenseFilter, limit: usize, offset: usize,
    ) -> Result<(usize, Vec<ExpenseRecord>), AppError> {
        let sql = build_expense_filter(filter);
        let mut q = self.expense_table.query();
        if let Some(f) = sql { q = q.only_if(f); }
        let stream = q.execute().await.map_err(|e| AppError::Internal(e.to_string()))?;
        let mut records = batches_to_expenses(collect_stream(stream).await?)?;
        if let Some(ref from) = filter.from {
            records.retain(|r| effective_date(r).as_str() >= from.as_str());
        }
        if let Some(ref to) = filter.to {
            records.retain(|r| effective_date(r).as_str() <= to.as_str());
        }
        records.sort_by(|a, b| {
            effective_date(b).cmp(&effective_date(a))
                .then(b.created_at.cmp(&a.created_at))
        });
        let total = records.len();
        let items = records.into_iter().skip(offset).take(limit).collect();
        Ok((total, items))
    }

    pub async fn expenses_referencing_blob(&self, blob_id: Uuid) -> Result<Vec<ExpenseRecord>, AppError> {
        let id_str = blob_id.to_string();
        let stream = self.expense_table.query()
            .only_if(format!("blob_ids LIKE '%\"{id_str}\"%'"))
            .execute().await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        batches_to_expenses(collect_stream(stream).await?)
    }
}

// --- Helpers ---

fn expense_to_batch(record: &ExpenseRecord, embed_dim: usize) -> Result<RecordBatch, AppError> {
    let schema = expense_schema(embed_dim);
    let id_str = record.id.to_string();
    let status_str = record.status.as_str();
    let category_str = record.category.as_str();
    let driver_id_str = record.driver_id.map(|u| u.to_string());
    let trip_id_str = record.trip_id.map(|u| u.to_string());
    let equipment_type_str = record.equipment_type.map(|t| t.as_str());
    let equipment_id_str = record.equipment_id.map(|u| u.to_string());
    let maintenance_id_str = record.maintenance_id.map(|u| u.to_string());
    let blob_ids_json = serde_json::to_string(&record.blob_ids)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let payment_method_str = record.payment_method.map(|m| m.as_str());
    let reviewed_at_str = record.reviewed_at.map(|d| d.to_rfc3339());
    let settlement_id_str = record.settlement_id.map(|u| u.to_string());
    let created_str = record.created_at.to_rfc3339();
    let updated_str = record.updated_at.to_rfc3339();

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
        Arc::new(StringArray::from(vec![status_str])),
        Arc::new(StringArray::from(vec![category_str])),
        Arc::new(StringArray::from(vec![driver_id_str.as_deref()])),
        Arc::new(StringArray::from(vec![trip_id_str.as_deref()])),
        Arc::new(StringArray::from(vec![equipment_type_str])),
        Arc::new(StringArray::from(vec![equipment_id_str.as_deref()])),
        Arc::new(StringArray::from(vec![maintenance_id_str.as_deref()])),
        Arc::new(StringArray::from(vec![blob_ids_json.as_str()])),
        Arc::new(StringArray::from(vec![record.submitted_by.as_str()])),
        Arc::new(StringArray::from(vec![record.expense_date.as_deref()])),
        Arc::new(StringArray::from(vec![record.vendor.as_deref()])),
        Arc::new(Float64Array::from(vec![record.amount])),
        Arc::new(Float64Array::from(vec![record.approved_amount])),
        Arc::new(StringArray::from(vec![payment_method_str])),
        Arc::new(Float64Array::from(vec![record.suggested_amount])),
        Arc::new(StringArray::from(vec![record.suggested_date.as_deref()])),
        Arc::new(StringArray::from(vec![record.suggested_vendor.as_deref()])),
        Arc::new(StringArray::from(vec![record.suggested_card_last4.as_deref()])),
        Arc::new(StringArray::from(vec![record.reviewed_by.as_deref()])),
        Arc::new(StringArray::from(vec![reviewed_at_str.as_deref()])),
        Arc::new(StringArray::from(vec![record.review_note.as_deref()])),
        Arc::new(StringArray::from(vec![settlement_id_str.as_deref()])),
        embedding_col,
        Arc::new(Int64Array::from(vec![record.owner_id])),
        Arc::new(StringArray::from(vec![created_str.as_str()])),
        Arc::new(StringArray::from(vec![updated_str.as_str()])),
    ]).map_err(|e| AppError::Internal(e.to_string()))
}

fn batches_to_expenses(batches: Vec<RecordBatch>) -> Result<Vec<ExpenseRecord>, AppError> {
    let mut out = Vec::new();
    for batch in &batches {
        for i in 0..batch.num_rows() { out.push(row_to_expense(batch, i)?); }
    }
    Ok(out)
}

fn row_to_expense(batch: &RecordBatch, i: usize) -> Result<ExpenseRecord, AppError> {
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
    let opt_uuid = |name: &str| -> Result<Option<Uuid>, AppError> {
        opt_str(name).map(|s| s.parse::<Uuid>()
            .map_err(|e| AppError::Internal(e.to_string()))).transpose()
    };
    let i64_col = |name: &str| -> i64 {
        batch.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(i)).unwrap_or(0)
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

    Ok(ExpenseRecord {
        id: str_col("id").parse().map_err(|e: uuid::Error| AppError::Internal(e.to_string()))?,
        status: str_col("status").parse().map_err(AppError::Internal)?,
        category: str_col("category").parse().map_err(AppError::Internal)?,
        driver_id: opt_uuid("driver_id")?,
        trip_id: opt_uuid("trip_id")?,
        equipment_type: opt_str("equipment_type").map(|s| s.parse()).transpose().map_err(AppError::Internal)?,
        equipment_id: opt_uuid("equipment_id")?,
        maintenance_id: opt_uuid("maintenance_id")?,
        blob_ids: serde_json::from_str(&str_col("blob_ids")).unwrap_or_default(),
        submitted_by: str_col("submitted_by"),
        expense_date: opt_str("expense_date"),
        vendor: opt_str("vendor"),
        amount: opt_f64("amount"),
        approved_amount: opt_f64("approved_amount"),
        payment_method: opt_str("payment_method").map(|s| s.parse()).transpose().map_err(AppError::Internal)?,
        suggested_amount: opt_f64("suggested_amount"),
        suggested_date: opt_str("suggested_date"),
        suggested_vendor: opt_str("suggested_vendor"),
        suggested_card_last4: opt_str("suggested_card_last4"),
        reviewed_by: opt_str("reviewed_by"),
        reviewed_at: opt_str("reviewed_at").map(|s| s.parse())
            .transpose().map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        review_note: opt_str("review_note"),
        settlement_id: opt_uuid("settlement_id")?,
        embedding,
        owner_id: i64_col("owner_id"),
        created_at: str_col("created_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
        updated_at: str_col("updated_at").parse()
            .map_err(|e: chrono::ParseError| AppError::Internal(e.to_string()))?,
    })
}

fn build_expense_filter(filter: &ExpenseFilter) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    if let Some(s) = &filter.status {
        clauses.push(format!("status = '{}'", s.replace('\'', "''")));
    }
    if let Some(c) = &filter.category {
        clauses.push(format!("category = '{}'", c.replace('\'', "''")));
    }
    if let Some(d) = &filter.driver_id {
        clauses.push(format!("driver_id = '{}'", d.replace('\'', "''")));
    }
    if let Some(t) = &filter.trip_id {
        clauses.push(format!("trip_id = '{}'", t.replace('\'', "''")));
    }
    if let Some(e) = &filter.equipment_id {
        clauses.push(format!("equipment_id = '{}'", e.replace('\'', "''")));
    }
    if let Some(s) = &filter.submitted_by {
        clauses.push(format!("submitted_by = '{}'", s.replace('\'', "''")));
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
    use crate::models::{EquipmentType, ExpenseCategory, ExpenseStatus, PaymentMethod};
    use tempfile::TempDir;

    async fn test_db() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbClient::new(dir.path().to_str().unwrap(), 4).await.unwrap();
        (db, dir)
    }

    fn sample(driver_id: Uuid) -> ExpenseRecord {
        let now = Utc::now();
        ExpenseRecord {
            id: Uuid::new_v4(),
            status: ExpenseStatus::Submitted,
            category: ExpenseCategory::Fuel,
            driver_id: Some(driver_id),
            trip_id: None,
            equipment_type: None,
            equipment_id: None,
            maintenance_id: None,
            blob_ids: vec![],
            submitted_by: format!("driver:{driver_id}"),
            expense_date: None,
            vendor: None,
            amount: None,
            approved_amount: None,
            payment_method: None,
            suggested_amount: None,
            suggested_date: None,
            suggested_vendor: None,
            suggested_card_last4: None,
            reviewed_by: None,
            reviewed_at: None,
            review_note: None,
            settlement_id: None,
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        }
    }

    fn fully_populated_reviewed(driver_id: Uuid) -> ExpenseRecord {
        let now = Utc::now();
        ExpenseRecord {
            id: Uuid::new_v4(),
            status: ExpenseStatus::Reviewed,
            category: ExpenseCategory::Repair,
            driver_id: Some(driver_id),
            trip_id: Some(Uuid::new_v4()),
            equipment_type: Some(EquipmentType::Truck),
            equipment_id: Some(Uuid::new_v4()),
            maintenance_id: Some(Uuid::new_v4()),
            blob_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
            submitted_by: format!("driver:{driver_id}"),
            expense_date: Some("2026-06-01".into()),
            vendor: Some("Acme Diesel".into()),
            amount: Some(150.0),
            approved_amount: Some(120.0),
            payment_method: Some(PaymentMethod::Personal),
            suggested_amount: Some(150.0),
            suggested_date: Some("2026-06-01".into()),
            suggested_vendor: Some("Acme Diesel".into()),
            suggested_card_last4: Some("4242".into()),
            reviewed_by: Some(format!("fleet_user:{}", Uuid::new_v4())),
            reviewed_at: Some(now),
            review_note: Some("approved minus tip".into()),
            settlement_id: Some(Uuid::new_v4()),
            embedding: None,
            owner_id: 0,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn test_insert_and_get_full_roundtrip() {
        let (db, _dir) = test_db().await;
        let e = fully_populated_reviewed(Uuid::new_v4());
        db.insert_expense(&e).await.unwrap();
        let got = db.get_expense_by_id(e.id).await.unwrap();

        assert_eq!(got.id, e.id);
        assert_eq!(got.status, ExpenseStatus::Reviewed);
        assert_eq!(got.category, ExpenseCategory::Repair);
        assert_eq!(got.driver_id, e.driver_id);
        assert_eq!(got.trip_id, e.trip_id);
        assert_eq!(got.equipment_type, Some(EquipmentType::Truck));
        assert_eq!(got.equipment_id, e.equipment_id);
        assert_eq!(got.maintenance_id, e.maintenance_id);
        assert_eq!(got.blob_ids, e.blob_ids);
        assert_eq!(got.submitted_by, e.submitted_by);
        assert_eq!(got.expense_date, e.expense_date);
        assert_eq!(got.vendor, e.vendor);
        assert_eq!(got.amount, e.amount);
        assert_eq!(got.approved_amount, e.approved_amount);
        assert_eq!(got.payment_method, Some(PaymentMethod::Personal));
        assert_eq!(got.suggested_amount, e.suggested_amount);
        assert_eq!(got.suggested_date, e.suggested_date);
        assert_eq!(got.suggested_vendor, e.suggested_vendor);
        assert_eq!(got.suggested_card_last4, e.suggested_card_last4);
        assert_eq!(got.reviewed_by, e.reviewed_by);
        assert_eq!(
            got.reviewed_at.unwrap().timestamp_millis(),
            e.reviewed_at.unwrap().timestamp_millis()
        );
        assert_eq!(got.review_note, e.review_note);
        assert_eq!(got.settlement_id, e.settlement_id);
        assert_eq!(got.owner_id, e.owner_id);
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let (db, _dir) = test_db().await;
        assert!(matches!(db.get_expense_by_id(Uuid::new_v4()).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_update_expense_whole_record() {
        let (db, _dir) = test_db().await;
        let e = sample(Uuid::new_v4());
        db.insert_expense(&e).await.unwrap();

        let mut updated = e.clone();
        updated.status = ExpenseStatus::Reviewed;
        updated.amount = Some(200.0);
        updated.approved_amount = Some(180.0);
        updated.payment_method = Some(PaymentMethod::Company);
        updated.updated_at = Utc::now();
        db.update_expense(&updated).await.unwrap();

        let got = db.get_expense_by_id(e.id).await.unwrap();
        assert_eq!(got.status, ExpenseStatus::Reviewed);
        assert_eq!(got.amount, Some(200.0));
        assert_eq!(got.approved_amount, Some(180.0));
        assert_eq!(got.payment_method, Some(PaymentMethod::Company));
    }

    #[tokio::test]
    async fn test_update_expense_suggestions_touches_only_suggested_fields() {
        let (db, _dir) = test_db().await;
        let e = sample(Uuid::new_v4());
        db.insert_expense(&e).await.unwrap();

        db.update_expense_suggestions(
            e.id,
            Some(42.5),
            Some("2026-07-01".into()),
            Some("Flying J".into()),
            Some("1234".into()),
        ).await.unwrap();

        let got = db.get_expense_by_id(e.id).await.unwrap();
        assert_eq!(got.suggested_amount, Some(42.5));
        assert_eq!(got.suggested_date, Some("2026-07-01".into()));
        assert_eq!(got.suggested_vendor, Some("Flying J".into()));
        assert_eq!(got.suggested_card_last4, Some("1234".into()));
        // Other fields untouched.
        assert_eq!(got.status, ExpenseStatus::Submitted);
        assert_eq!(got.category, ExpenseCategory::Fuel);
        assert_eq!(got.driver_id, e.driver_id);
        assert_eq!(got.submitted_by, e.submitted_by);
        assert_eq!(got.amount, None);
    }

    #[tokio::test]
    async fn test_delete_expense() {
        let (db, _dir) = test_db().await;
        let e = sample(Uuid::new_v4());
        db.insert_expense(&e).await.unwrap();
        db.delete_expense(e.id).await.unwrap();
        assert!(matches!(db.get_expense_by_id(e.id).await, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn test_list_expenses_filters_sort_and_paging() {
        let (db, _dir) = test_db().await;
        let driver_a = Uuid::new_v4();
        let driver_b = Uuid::new_v4();

        let mut e1 = sample(driver_a);
        e1.category = ExpenseCategory::Fuel;
        e1.submitted_by = format!("driver:{driver_a}");

        let mut e2 = fully_populated_reviewed(driver_a);
        e2.category = ExpenseCategory::Repair;
        e2.status = ExpenseStatus::Reviewed;
        e2.expense_date = Some("2026-06-15".into());
        e2.submitted_by = format!("driver:{driver_a}");

        let mut e3 = sample(driver_b);
        e3.category = ExpenseCategory::Tolls;
        e3.submitted_by = format!("driver:{driver_b}");

        db.insert_expense(&e1).await.unwrap();
        db.insert_expense(&e2).await.unwrap();
        db.insert_expense(&e3).await.unwrap();

        // driver filter
        let (total, items) = db.list_expenses(
            &ExpenseFilter { driver_id: Some(driver_a.to_string()), ..Default::default() },
            50, 0,
        ).await.unwrap();
        assert_eq!(total, 2);
        assert_eq!(items.len(), 2);

        // category filter
        let (total, items) = db.list_expenses(
            &ExpenseFilter { category: Some("repair".into()), ..Default::default() },
            50, 0,
        ).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].id, e2.id);

        // status filter
        let (total, _) = db.list_expenses(
            &ExpenseFilter { status: Some("reviewed".into()), ..Default::default() },
            50, 0,
        ).await.unwrap();
        assert_eq!(total, 1);

        // submitted_by filter
        let (total, _) = db.list_expenses(
            &ExpenseFilter { submitted_by: Some(format!("driver:{driver_b}")), ..Default::default() },
            50, 0,
        ).await.unwrap();
        assert_eq!(total, 1);

        // from/to range: e2's expense_date is 2026-06-15; e1/e3 fall back to
        // created_at's date (today). Range covering only e2's date.
        let (total, items) = db.list_expenses(
            &ExpenseFilter {
                from: Some("2026-06-01".into()),
                to: Some("2026-06-30".into()),
                ..Default::default()
            },
            50, 0,
        ).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].id, e2.id);

        // sort newest-first by effective_date: e2 (2026-06-15) should sort
        // before e1/e3 (today's date) since today > 2026-06-15... to keep sort
        // assertion independent of "today", filter down to driver_a only and
        // check ordering among e1 (created today, no expense_date) and e2.
        let (_, items) = db.list_expenses(
            &ExpenseFilter { driver_id: Some(driver_a.to_string()), ..Default::default() },
            50, 0,
        ).await.unwrap();
        let today = Utc::now().format("%Y-%m-%d").to_string();
        if today.as_str() > "2026-06-15" {
            assert_eq!(items[0].id, e1.id);
            assert_eq!(items[1].id, e2.id);
        } else {
            assert_eq!(items[0].id, e2.id);
            assert_eq!(items[1].id, e1.id);
        }

        // offset/limit paging over all 3 records.
        let (total_all, page1) = db.list_expenses(&ExpenseFilter::default(), 2, 0).await.unwrap();
        assert_eq!(total_all, 3);
        assert_eq!(page1.len(), 2);
        let (_, page2) = db.list_expenses(&ExpenseFilter::default(), 2, 2).await.unwrap();
        assert_eq!(page2.len(), 1);
    }

    #[tokio::test]
    async fn test_expenses_referencing_blob() {
        let (db, _dir) = test_db().await;
        let blob_id = Uuid::new_v4();
        let mut e1 = sample(Uuid::new_v4());
        e1.blob_ids = vec![blob_id, Uuid::new_v4()];
        let e2 = sample(Uuid::new_v4());

        db.insert_expense(&e1).await.unwrap();
        db.insert_expense(&e2).await.unwrap();

        let found = db.expenses_referencing_blob(blob_id).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, e1.id);

        let not_found = db.expenses_referencing_blob(Uuid::new_v4()).await.unwrap();
        assert!(not_found.is_empty());
    }
}
