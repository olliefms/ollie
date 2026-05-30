// src/db/terminal_ops.rs
use std::collections::HashMap;
use std::sync::Arc;
use arrow_array::{
    Array, BooleanArray, Float64Array, Int64Array, RecordBatch,
    RecordBatchIterator, RecordBatchReader, StringArray,
};
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use uuid::Uuid;

use crate::db::{terminal_schema, DbClient};
use crate::error::AppError;
use crate::models::{TerminalListItem, TerminalRecord};

fn terminal_to_batch(r: &TerminalRecord) -> Result<RecordBatch, AppError> {
    let schema = terminal_schema();
    let id = r.id.to_string();
    let created = r.created_at.to_rfc3339();
    let updated = r.updated_at.to_rfc3339();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![id.as_str()])),
            Arc::new(StringArray::from(vec![r.name.as_str()])),
            Arc::new(StringArray::from(vec![r.address.as_deref()])),
            Arc::new(StringArray::from(vec![r.timezone.as_str()])),
            Arc::new(BooleanArray::from(vec![r.is_default])),
            Arc::new(Float64Array::from(vec![r.loaded_rate_per_mile])),
            Arc::new(Float64Array::from(vec![r.deadhead_rate_per_mile])),
            Arc::new(Float64Array::from(vec![r.extra_stop_fee])),
            Arc::new(Float64Array::from(vec![r.detention_rate_per_hour])),
            Arc::new(Int64Array::from(vec![r.free_dwell_minutes as i64])),
            Arc::new(Int64Array::from(vec![r.owner_id])),
            Arc::new(StringArray::from(vec![created.as_str()])),
            Arc::new(StringArray::from(vec![updated.as_str()])),
        ],
    )
    .map_err(|e| AppError::Internal(e.to_string()))
}

fn row_to_terminal(batch: &RecordBatch, i: usize) -> Result<TerminalRecord, AppError> {
    let s = |name: &str| -> String {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .map(|a| a.value(i).to_string())
            .unwrap_or_default()
    };
    let opt_s = |name: &str| -> Option<String> {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<StringArray>())
            .and_then(|a| {
                if a.is_null(i) {
                    None
                } else {
                    Some(a.value(i).to_string())
                }
            })
    };
    let f = |name: &str| -> f64 {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Float64Array>())
            .map(|a| a.value(i))
            .unwrap_or(0.0)
    };
    let i64c = |name: &str| -> i64 {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
            .map(|a| a.value(i))
            .unwrap_or(0)
    };
    let b = |name: &str| -> bool {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<BooleanArray>())
            .map(|a| a.value(i))
            .unwrap_or(false)
    };
    let parse_dt = |raw: String| {
        chrono::DateTime::parse_from_rfc3339(&raw)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now())
    };
    Ok(TerminalRecord {
        id: s("id")
            .parse()
            .map_err(|e| AppError::Internal(format!("{e}")))?,
        name: s("name"),
        address: opt_s("address"),
        timezone: s("timezone"),
        is_default: b("is_default"),
        loaded_rate_per_mile: f("loaded_rate_per_mile"),
        deadhead_rate_per_mile: f("deadhead_rate_per_mile"),
        extra_stop_fee: f("extra_stop_fee"),
        detention_rate_per_hour: f("detention_rate_per_hour"),
        free_dwell_minutes: i64c("free_dwell_minutes") as u32,
        owner_id: i64c("owner_id"),
        created_at: parse_dt(s("created_at")),
        updated_at: parse_dt(s("updated_at")),
    })
}

impl DbClient {
    async fn all_terminal_batches(&self) -> Result<Vec<RecordBatch>, AppError> {
        self.terminal_table
            .query()
            .execute()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    async fn upsert_terminal(&self, r: &TerminalRecord) -> Result<(), AppError> {
        let batch = terminal_to_batch(r)?;
        let schema = terminal_schema();
        let iter = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let reader: Box<dyn RecordBatchReader + Send> = Box::new(iter);
        let mut op = self.terminal_table.merge_insert(&["id"]);
        op.when_matched_update_all(None).when_not_matched_insert_all();
        op.execute(reader)
            .await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }

    pub async fn insert_terminal(&self, r: &TerminalRecord) -> Result<(), AppError> {
        self.upsert_terminal(r).await
    }

    pub async fn get_terminal_by_id(&self, id: Uuid) -> Result<TerminalRecord, AppError> {
        let id_s = id.to_string();
        let batches = self
            .terminal_table
            .query()
            .only_if(format!("id = '{id_s}'"))
            .execute()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        for batch in &batches {
            if batch.num_rows() > 0 {
                return row_to_terminal(batch, 0);
            }
        }
        // NOTE: AppError::NotFound is a UNIT variant (src/error.rs) — no String payload.
        Err(AppError::NotFound)
    }

    pub async fn batch_get_terminals(
        &self,
        ids: &[Uuid],
    ) -> Result<HashMap<Uuid, TerminalRecord>, AppError> {
        let mut out = HashMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        for batch in self.all_terminal_batches().await? {
            for i in 0..batch.num_rows() {
                let t = row_to_terminal(&batch, i)?;
                if ids.contains(&t.id) {
                    out.insert(t.id, t);
                }
            }
        }
        Ok(out)
    }

    pub async fn default_terminal(&self) -> Result<TerminalRecord, AppError> {
        for batch in self.all_terminal_batches().await? {
            for i in 0..batch.num_rows() {
                let t = row_to_terminal(&batch, i)?;
                if t.is_default {
                    return Ok(t);
                }
            }
        }
        Err(AppError::Internal("no default terminal found".into()))
    }

    pub async fn list_terminals(&self) -> Result<Vec<TerminalListItem>, AppError> {
        let mut out = Vec::new();
        for batch in self.all_terminal_batches().await? {
            for i in 0..batch.num_rows() {
                out.push(TerminalListItem::from(row_to_terminal(&batch, i)?));
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// If `r.is_default` is true, clears `is_default` on all other terminals first.
    pub async fn set_terminal(&self, r: &TerminalRecord) -> Result<(), AppError> {
        if r.is_default {
            let others: Vec<TerminalRecord> = {
                let mut v = Vec::new();
                for batch in self.all_terminal_batches().await? {
                    for i in 0..batch.num_rows() {
                        let t = row_to_terminal(&batch, i)?;
                        if t.id != r.id && t.is_default {
                            v.push(t);
                        }
                    }
                }
                v
            };
            for mut o in others {
                o.is_default = false;
                o.updated_at = Utc::now();
                self.upsert_terminal(&o).await?;
            }
        }
        self.upsert_terminal(r).await
    }

    pub async fn count_drivers_for_terminal(
        &self,
        terminal_id: Uuid,
    ) -> Result<usize, AppError> {
        let n = self
            .driver_table
            .query()
            .only_if(format!("terminal_id = '{}'", terminal_id))
            .execute()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
            .iter()
            .map(|b| b.num_rows())
            .sum();
        Ok(n)
    }

    pub async fn delete_terminal(&self, id: Uuid) -> Result<(), AppError> {
        self.terminal_table
            .delete(&format!("id = '{}'", id))
            .await
            .map(|_| ())
            .map_err(|e| AppError::Internal(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn client() -> (DbClient, TempDir) {
        let dir = TempDir::new().unwrap();
        let c = DbClient::new(dir.path().to_str().unwrap(), 4)
            .await
            .unwrap();
        (c, dir)
    }

    #[tokio::test]
    async fn seeded_default_terminal_exists() {
        let (c, _d) = client().await;
        let def = c.default_terminal().await.unwrap();
        assert_eq!(def.name, "Default");
        assert_eq!(def.free_dwell_minutes, 120);
        assert_eq!(def.loaded_rate_per_mile, 0.0);
    }

    #[tokio::test]
    async fn insert_get_roundtrip_and_single_default() {
        let (c, _d) = client().await;
        let mut t = TerminalRecord {
            id: Uuid::new_v4(),
            name: "West".into(),
            address: Some("1 A St".into()),
            timezone: "America/Los_Angeles".into(),
            is_default: true,
            loaded_rate_per_mile: 0.55,
            deadhead_rate_per_mile: 0.40,
            extra_stop_fee: 50.0,
            detention_rate_per_hour: 25.0,
            free_dwell_minutes: 90,
            owner_id: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        c.set_terminal(&t).await.unwrap();
        let got = c.get_terminal_by_id(t.id).await.unwrap();
        assert_eq!(got.name, "West");
        assert_eq!(got.free_dwell_minutes, 90);
        // The originally-seeded Default must have been un-defaulted.
        let defaults: Vec<_> = c
            .list_terminals()
            .await
            .unwrap()
            .into_iter()
            .filter(|x| x.is_default)
            .collect();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].id, t.id);
        // update
        t.name = "West Yard".into();
        t.updated_at = Utc::now();
        c.set_terminal(&t).await.unwrap();
        assert_eq!(
            c.get_terminal_by_id(t.id).await.unwrap().name,
            "West Yard"
        );
    }
}
