// src/services/maintenance.rs
//! Periodic LanceDB compaction and version pruning (#403).
//!
//! Lance is append-only: every write — insert *and* the `merge_insert` upsert
//! every update goes through — lands in a new fragment and leaves a new version
//! manifest behind. Nothing reclaims either, so file count tracks *writes ever
//! made*, not rows currently stored: a row updated 50 times contributes 50
//! fragments. A single-truck fleet reached 2,069 fragments across 2,069 blob
//! rows and 30,371 files under `/data/lancedb`, against a container default
//! `ulimit -n` of 1024. Reads then fail intermittently with "Too many open
//! files", and which read fails first is arbitrary.
//!
//! Compaction runs **in-process**, against the same `lance` the server is linked
//! with. That is the whole reason this exists rather than a documented cron:
//! compacting externally with `pylance` 9.0.0 silently upgraded four tables from
//! storage version 2.0 to 2.1 in testing, and a server built against
//! `lance-io 6.0.0` may not be able to read that back. Anyone hitting this in
//! the wild reaches for `pylance` first, so the safe path has to be the built-in
//! one.

use crate::db::DbClient;
use lancedb::table::{CompactionOptions, OptimizeAction};
use lancedb::Table;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

/// Default gap between scheduled passes (`OLLIE_MAINTENANCE_INTERVAL_SECS`).
pub const DEFAULT_INTERVAL_SECS: u64 = 6 * 60 * 60;

/// Default compaction target (`OLLIE_MAINTENANCE_TARGET_ROWS_PER_FRAGMENT`),
/// matching lance's own. Fragments smaller than this are compaction candidates;
/// every table here is orders of magnitude below one fragment's worth, so the
/// practical effect is "collapse the table into a single fragment".
pub const DEFAULT_TARGET_ROWS_PER_FRAGMENT: usize = 1_048_576;

/// Delay before the first pass. Long enough not to contend with index creation
/// and the startup requeue burst, short enough that an instance restarted *to
/// escape* fd exhaustion gets relief now rather than one interval from now.
const FIRST_PASS_DELAY: Duration = Duration::from_secs(60);

/// How much version history the prune pass keeps. Ollie never checks out an old
/// version, so retention buys nothing but disk — and manifests were the larger
/// half of the file count in #403 (2,813 manifests against 2,940 data files),
/// because *every* write leaves one while only an insert adds a fragment.
///
/// This window is NOT what protects a concurrent reader — see the ordering
/// comment in `run_one`, which is. Lance's 7-day threshold isn't either: it
/// covers only files *no* manifest references (a write that may still be in
/// flight), never a file referenced by the old manifest being deleted.
pub const PRUNE_OLDER_THAN_HOURS: i64 = 24;

/// One dataset's fragmentation, and what the pass did about it.
///
/// The `*_after` fields are absent on a dry run — that shape *is* the
/// "before it becomes an outage" report the issue asks for.
#[derive(Debug, Clone, Serialize)]
pub struct DatasetReport {
    pub table: &'static str,
    pub rows: usize,
    pub fragments: usize,
    pub versions: usize,
    /// Fragments per row. `1.0` is the #403 state — every row its own file.
    /// A compacted table is a small fraction of that.
    pub fragments_per_row: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fragments_after: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versions_after: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_removed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_removed: Option<u64>,
    /// Set when this dataset failed. The pass continues with the rest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MaintenanceReport {
    /// False for a dry run — nothing on disk was touched.
    pub applied: bool,
    pub duration_ms: u128,
    pub datasets: Vec<DatasetReport>,
}

impl MaintenanceReport {
    pub fn fragments_before(&self) -> usize {
        self.datasets.iter().map(|d| d.fragments).sum()
    }

    /// Sums only the datasets that were measured after the pass, so a failed
    /// table is absent from this rather than counted at its pre-pass value.
    pub fn fragments_after(&self) -> usize {
        self.datasets.iter().filter_map(|d| d.fragments_after).sum()
    }

    pub fn failures(&self) -> impl Iterator<Item = &DatasetReport> {
        self.datasets.iter().filter(|d| d.error.is_some())
    }
}

/// Run one maintenance pass over every dataset.
///
/// `apply = false` only measures. `apply = true` prunes old versions and then
/// compacts, one table at a time — that order is load-bearing, see `run_one`.
/// A table that fails is recorded in its own report and the pass continues:
/// one bad dataset must not stop the others from reclaiming their descriptors,
/// which is the entire point of running.
///
/// Returns `None` when a mutating pass is already in flight.
pub async fn run(
    db: &DbClient,
    apply: bool,
    target_rows_per_fragment: usize,
) -> Option<MaintenanceReport> {
    run_with_retention(
        db,
        apply,
        target_rows_per_fragment,
        chrono::Duration::hours(PRUNE_OLDER_THAN_HOURS),
    )
    .await
}

/// [`run`] with an explicit version-retention window. Production goes through
/// `run`, which owns the policy; this exists so a test can prune history it just
/// created instead of waiting a day for it to age out.
pub async fn run_with_retention(
    db: &DbClient,
    apply: bool,
    target_rows_per_fragment: usize,
    prune_older_than: chrono::Duration,
) -> Option<MaintenanceReport> {
    // Only the mutating pass is serialized. Reading fragment counts during a
    // compaction is harmless, and refusing to answer "how fragmented am I"
    // during the operation that fixes it would be unhelpful.
    let _guard = if apply {
        Some(db.maintenance_lock.try_lock().ok()?)
    } else {
        None
    };

    let started = std::time::Instant::now();
    let mut datasets = Vec::new();
    for (name, table) in db.tables() {
        datasets.push(run_one(name, table, apply, target_rows_per_fragment, prune_older_than).await);
    }
    Some(MaintenanceReport {
        applied: apply,
        duration_ms: started.elapsed().as_millis(),
        datasets,
    })
}

async fn run_one(
    name: &'static str,
    table: &Table,
    apply: bool,
    target_rows_per_fragment: usize,
    prune_older_than: chrono::Duration,
) -> DatasetReport {
    let mut report = DatasetReport {
        table: name,
        rows: 0,
        fragments: 0,
        versions: 0,
        fragments_per_row: 0.0,
        fragments_after: None,
        versions_after: None,
        files_removed: None,
        bytes_removed: None,
        error: None,
    };

    match measure(table).await {
        Ok((rows, fragments, versions)) => {
            report.rows = rows;
            report.fragments = fragments;
            report.versions = versions;
            report.fragments_per_row =
                if rows == 0 { 0.0 } else { fragments as f64 / rows as f64 };
        }
        Err(e) => {
            report.error = Some(e);
            return report;
        }
    }

    if !apply {
        return report;
    }

    // ---- Prune BEFORE compact. The order is load-bearing. ----
    //
    // Compaction supersedes the current version without deleting anything; the
    // prune is the only step that removes files. Run it second and it deletes
    // the files compaction *just* orphaned — and lance classifies a data file
    // referenced only by manifests it is deleting as "verified", so it goes
    // unconditionally (`cleanup.rs`; the 7-day threshold guards unreferenced
    // files only, not this). Any request already scanning the pre-compaction
    // snapshot then dies mid-read with "No such file or directory". That needs
    // no unusual timing: a table whose last write predates the retention window
    // — routine for terminals/trucks/drivers, and true of *every* table on the
    // first pass 60s after a restart — becomes prunable the instant compaction
    // supersedes it.
    //
    // Pruning first, the live snapshot is still the latest version, so its files
    // are referenced and survive. What compaction orphans is reclaimed on the
    // next pass an interval later, by which point no reader can still hold it.
    // That deferral is what the raised `nofile` headroom is for.
    let prune = OptimizeAction::Prune {
        older_than: Some(prune_older_than),
        delete_unverified: None,
        error_if_tagged_old_versions: None,
    };
    match table.optimize(prune).await {
        Ok(stats) => report.bytes_removed = stats.prune.map(|p| p.bytes_removed),
        // Not fatal to the pass: compaction is the half that reclaims
        // descriptors, and it is safe to attempt on an unpruned dataset.
        Err(e) => push_error(&mut report, format!("prune: {e}")),
    }

    let compact = OptimizeAction::Compact {
        options: CompactionOptions {
            target_rows_per_fragment,
            ..Default::default()
        },
        remap_options: None,
    };
    match table.optimize(compact).await {
        Ok(stats) => report.files_removed = stats.compaction.map(|m| m.files_removed),
        Err(e) => push_error(&mut report, format!("compact: {e}")),
    }

    match measure(table).await {
        Ok((_, fragments, versions)) => {
            report.fragments_after = Some(fragments);
            report.versions_after = Some(versions);
        }
        Err(e) => push_error(&mut report, format!("post-measure: {e}")),
    }
    report
}

/// Record a failure without dropping one already there — a pass can fail at
/// more than one step and the operator needs to see all of them.
fn push_error(report: &mut DatasetReport, msg: String) {
    match &mut report.error {
        Some(existing) => {
            existing.push_str("; ");
            existing.push_str(&msg);
        }
        None => report.error = Some(msg),
    }
}

/// `(rows, fragments, versions)`.
///
/// `count_fragments` is a metadata read; `Table::stats()` would also yield it
/// but computes per-field byte sizes on the way, which is a scan.
async fn measure(table: &Table) -> Result<(usize, usize, usize), String> {
    let rows = table.count_rows(None).await.map_err(|e| e.to_string())?;
    let native = table
        .as_native()
        // Only a remote (LanceDB Cloud) table is not native, which is not a
        // deployment ollie has. Say so rather than reporting zero fragments —
        // a silent 0 would read as "perfectly compacted".
        .ok_or_else(|| "fragment count unavailable: table is not a native dataset".to_string())?;
    let fragments = native.count_fragments().await.map_err(|e| e.to_string())?;
    let versions = table.list_versions().await.map_err(|e| e.to_string())?.len();
    Ok((rows, fragments, versions))
}

/// Spawn the periodic maintenance loop. A zero `interval` disables it.
///
/// Belongs behind the accept loop with every other data-proportional startup
/// task (#404): compaction time scales with the fragment backlog, which is
/// exactly what a freshly restarted, badly fragmented instance has.
pub fn spawn(db: Arc<DbClient>, interval: Duration, target_rows_per_fragment: usize) {
    if interval.is_zero() {
        tracing::info!("lancedb maintenance disabled (OLLIE_MAINTENANCE_INTERVAL_SECS=0)");
        return;
    }
    tokio::spawn(async move {
        let mut delay = FIRST_PASS_DELAY;
        loop {
            tokio::time::sleep(delay).await;
            delay = interval;
            // Each pass gets its own task so a panic inside lance costs one pass
            // and gets logged, rather than silently ending the loop for the life
            // of the process — the failure mode that made #410 invisible.
            let db = db.clone();
            let pass = tokio::spawn(async move { run(&db, true, target_rows_per_fragment).await });
            match pass.await {
                Ok(Some(report)) => log_pass(&report),
                Ok(None) => tracing::debug!("lancedb maintenance skipped: a pass is already running"),
                Err(e) => tracing::error!("lancedb maintenance pass ended abnormally: {e}"),
            }
        }
    });
}

fn log_pass(report: &MaintenanceReport) {
    for failed in report.failures() {
        tracing::warn!(
            "lancedb maintenance failed for {}: {}",
            failed.table,
            failed.error.as_deref().unwrap_or("")
        );
    }
    tracing::info!(
        "lancedb maintenance: {} -> {} fragments across {} datasets in {} ms",
        report.fragments_before(),
        report.fragments_after(),
        report.datasets.len(),
        report.duration_ms,
    );
}
