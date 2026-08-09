// tests/it/maintenance_test.rs
//! #403: LanceDB datasets never compact on their own.
//!
//! Every insert lands in its own fragment and nothing ever merges them, so a
//! table settles at one file per row; every write on top of that — updates
//! included — leaves another data file and version manifest that nothing
//! reclaims. Left alone the two together exhaust the process fd limit and reads
//! start failing with "Too many open files".
//!
//! The critical path is data integrity *through* a compaction: the pass must
//! collapse fragments without losing or corrupting a single row.

use chrono::Utc;
use ollie::{
    db::DbClient,
    models::{BlobRecord, BlobStatus},
    services::maintenance,
};
use tempfile::TempDir;
use uuid::Uuid;

const EMBED_DIM: usize = 4;
const ROWS: usize = 24;

fn blob(n: usize) -> BlobRecord {
    let now = Utc::now();
    BlobRecord {
        id: Uuid::new_v4(),
        owner_id: 0,
        checksum: format!("sum-{n}"),
        name: format!("doc-{n}.pdf"),
        mime_type: "application/pdf".into(),
        size: n as i64,
        status: BlobStatus::Pending,
        error: None,
        summary: None,
        tags: vec![],
        embedding: None,
        created_at: now,
        updated_at: now,
        visibility: Default::default(),
        uploaded_by: None,
    }
}

async fn open_db(dir: &TempDir) -> DbClient {
    DbClient::new(dir.path().to_str().unwrap(), EMBED_DIM).await.unwrap()
}

fn blobs_report(report: &maintenance::MaintenanceReport) -> &maintenance::DatasetReport {
    report
        .datasets
        .iter()
        .find(|d| d.table == "blobs")
        .expect("blobs dataset must be in the report")
}

/// The headline of #403: one fragment per row, on every table, because nothing
/// ever merges them. Compaction has to collapse them and hand back every row
/// exactly as it went in.
#[tokio::test]
async fn test_compaction_collapses_fragments_and_preserves_every_row() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir).await;

    let mut records: Vec<BlobRecord> = (0..ROWS).map(blob).collect();
    for r in &records {
        db.insert(r).await.unwrap();
    }
    // Updates are the half of #403 that surprises people. They leave the
    // fragment count alone — `merge_insert` writes a replacement fragment and
    // tombstones the one it supersedes — but each still leaves a data file and
    // a version manifest behind, which is why the reported instance carried
    // 2,940 data files and 2,813 manifests for only 2,069 rows. Compaction
    // reclaims the first count, the prune reclaims the second.
    for r in records.iter_mut().take(ROWS / 2) {
        r.status = BlobStatus::Ready;
        r.summary = Some(format!("summary for {}", r.name));
        db.mark_ready(r.id, r.summary.clone(), Some(vec![0.1; EMBED_DIM])).await.unwrap();
    }

    let before = maintenance::run(&db, false, 1_048_576).await.expect("dry run");
    let blobs = blobs_report(&before);
    assert!(!before.applied, "dry run must not report itself as applied");
    assert_eq!(blobs.rows, ROWS, "every insert should be one row");
    assert!(
        blobs.fragments >= ROWS,
        "the fragment-per-insert behaviour this fixes no longer reproduces: \
         {} fragments for {ROWS} rows",
        blobs.fragments,
    );
    assert!(
        blobs.fragments_per_row >= 1.0,
        "fragments_per_row should expose the ratio, got {}",
        blobs.fragments_per_row,
    );
    assert!(
        blobs.versions > ROWS,
        "every write should leave a version manifest: {} versions for {ROWS} inserts \
         plus {} updates",
        blobs.versions,
        ROWS / 2,
    );
    assert!(blobs.fragments_after.is_none(), "a dry run has no after-state");

    // Zero retention so the prune has something to remove — production keeps 24h
    // and would find every manifest here too young to touch.
    let after = maintenance::run_with_retention(&db, true, 1_048_576, chrono::Duration::zero())
        .await
        .expect("apply run");
    let blobs = blobs_report(&after);
    assert!(after.applied);
    assert!(blobs.error.is_none(), "compaction failed: {:?}", blobs.error);
    let compacted = blobs.fragments_after.expect("apply must measure the after-state");
    assert!(
        compacted < blobs.fragments,
        "compaction reclaimed nothing: {} -> {compacted} fragments",
        blobs.fragments,
    );
    let versions_after = blobs.versions_after.expect("apply must measure the after-state");
    assert!(
        versions_after < blobs.versions,
        "prune reclaimed no version history: {} -> {versions_after} versions",
        blobs.versions,
    );

    // The part that actually matters. Every row must survive the rewrite intact,
    // including the ones that were updated through merge_insert.
    assert_eq!(db.blob_table.count_rows(None).await.unwrap(), ROWS);
    for (i, r) in records.iter().enumerate() {
        let got = db.get_by_id(r.id).await.expect("row survived compaction");
        assert_eq!(got.name, r.name);
        assert_eq!(got.checksum, r.checksum);
        assert_eq!(got.size, r.size);
        if i < ROWS / 2 {
            assert_eq!(got.status, BlobStatus::Ready, "{} lost its update", r.name);
            assert_eq!(got.summary.as_deref(), Some(format!("summary for {}", r.name).as_str()));
        } else {
            assert_eq!(got.status, BlobStatus::Pending);
            assert!(got.summary.is_none());
        }
    }

    // Idempotent: a second pass over an already-compacted store is a no-op, not
    // a rewrite. The scheduler runs this every few hours forever.
    let again = maintenance::run(&db, true, 1_048_576).await.expect("second apply run");
    let blobs = blobs_report(&again);
    assert!(blobs.error.is_none(), "second pass failed: {:?}", blobs.error);
    assert_eq!(blobs.fragments_after, Some(compacted), "second pass should have nothing to do");
    assert_eq!(db.blob_table.count_rows(None).await.unwrap(), ROWS);
}

/// The dry run is the "see it before it becomes an outage" path. It must not
/// touch the store — an operator checking fragmentation is not consenting to a
/// rewrite.
#[tokio::test]
async fn test_dry_run_leaves_the_datasets_alone() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir).await;
    for n in 0..8 {
        db.insert(&blob(n)).await.unwrap();
    }

    let first = maintenance::run(&db, false, 1_048_576).await.expect("dry run");
    let second = maintenance::run(&db, false, 1_048_576).await.expect("dry run");
    for (a, b) in first.datasets.iter().zip(second.datasets.iter()) {
        assert_eq!(a.table, b.table);
        assert_eq!(a.fragments, b.fragments, "{} was compacted by a dry run", a.table);
        assert_eq!(a.versions, b.versions, "{} was pruned by a dry run", a.table);
        assert!(b.fragments_after.is_none());
        assert!(b.files_removed.is_none());
        assert!(b.bytes_removed.is_none());
    }
}

/// The pass must prune BEFORE it compacts, or it deletes the files it just
/// orphaned out from under whatever is mid-scan.
///
/// Compaction supersedes the current version without deleting anything; only the
/// prune deletes. Run the prune second and the pre-compaction data files are
/// referenced solely by the manifest it is removing, which makes them
/// "verified" in lance's terms and deletes them unconditionally — lance's 7-day
/// threshold covers unreferenced files, not these. A request already scanning
/// that snapshot then dies with "No such file or directory": the exact
/// intermittent read failure #403 exists to remove.
///
/// Zero retention makes it deterministic here; in production it needs only a
/// table whose last write predates the 24h window, which is every table on the
/// first pass after a restart.
#[tokio::test]
async fn test_pass_does_not_delete_files_an_in_flight_read_is_using() {
    use futures::TryStreamExt;
    use lancedb::query::ExecutableQuery;

    let dir = TempDir::new().unwrap();
    let db = open_db(&dir).await;
    // Enough fragments that the scan is still resolving files when the pass runs.
    const SCANNED: usize = 200;
    for n in 0..SCANNED {
        db.insert(&blob(n)).await.unwrap();
    }

    // Open the scan against the pre-pass snapshot and deliberately do not drain
    // it — this stands in for a request in flight when maintenance fires.
    let stream = db.blob_table.query().execute().await.expect("scan started");

    maintenance::run_with_retention(&db, true, 1_048_576, chrono::Duration::zero())
        .await
        .expect("apply run");

    let batches = stream
        .try_collect::<Vec<_>>()
        .await
        .expect("in-flight scan lost its files to the prune — the pass must prune before it compacts");
    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(rows, SCANNED, "in-flight scan came back short");
}

/// A table missing from `DbClient::tables()` never compacts — it leaks
/// descriptors forever and nothing says so. This is the omission guard.
#[tokio::test]
async fn test_tables_covers_every_dataset() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir).await;

    let mut listed: Vec<String> = db.tables().iter().map(|(name, _)| name.to_string()).collect();
    let mut on_disk = db.dataset_names().await.unwrap();
    listed.sort();
    on_disk.sort();
    assert_eq!(
        listed, on_disk,
        "DbClient::tables() and the datasets on disk disagree — a table added to \
         DbClient without a tables() entry never compacts (#403)",
    );

    // And the report has to actually cover them, not just the list.
    let report = maintenance::run(&db, false, 1_048_576).await.expect("dry run");
    let mut reported: Vec<String> = report.datasets.iter().map(|d| d.table.to_string()).collect();
    reported.sort();
    assert_eq!(reported, on_disk);
    assert!(report.failures().next().is_none(), "no dataset should fail to measure");
}

/// Compaction rewrites fragments and remaps every index that points at them.
/// The production `blobs` table carries an IVF-PQ vector index (2,069 rows in
/// the reported instance, well past the 256-row training floor), and a botched
/// remap corrupts `search_blobs` and facility dedup silently — nothing errors,
/// the results just stop being right. Every other test here sits below the
/// index floor, so this is the only one that exercises the remap at all.
#[tokio::test]
async fn test_compaction_preserves_vector_search_results() {
    const INDEXED_DIM: usize = 32;
    // Above MIN_IVFPQ_TRAINING_ROWS (256) — below it there is no index to remap.
    const INDEXED_ROWS: usize = 300;

    let dir = TempDir::new().unwrap();
    let db = DbClient::new(dir.path().to_str().unwrap(), INDEXED_DIM).await.unwrap();

    for n in 0..INDEXED_ROWS {
        let mut r = blob(n);
        r.status = BlobStatus::Ready;
        r.summary = Some(format!("summary {n}"));
        // Spread the vectors so nearest-neighbour order is well-defined rather
        // than an arbitrary tie-break that compaction could legitimately change.
        r.embedding = Some((0..INDEXED_DIM).map(|d| (n + d) as f32 / 100.0).collect());
        db.insert(&r).await.unwrap();
    }
    db.create_vector_index().await.unwrap();
    assert!(
        !db.blob_table.list_indices().await.unwrap().is_empty(),
        "no index was built — this test would pass vacuously",
    );

    let probe: Vec<f32> = (0..INDEXED_DIM).map(|d| (42 + d) as f32 / 100.0).collect();
    let before: Vec<String> = db.search(probe.clone(), None, &[], 5).await.unwrap()
        .into_iter().map(|i| i.name).collect();
    assert_eq!(before.len(), 5, "search returned nothing to compare against");

    let report = maintenance::run_with_retention(&db, true, 1_048_576, chrono::Duration::zero())
        .await
        .expect("apply run");
    let blobs = blobs_report(&report);
    assert!(blobs.error.is_none(), "compaction failed: {:?}", blobs.error);
    assert!(
        blobs.fragments_after.unwrap() < blobs.fragments,
        "nothing was compacted, so the remap was never exercised",
    );
    assert!(
        !db.blob_table.list_indices().await.unwrap().is_empty(),
        "compaction dropped the vector index",
    );

    let after: Vec<String> = db.search(probe, None, &[], 5).await.unwrap()
        .into_iter().map(|i| i.name).collect();
    assert_eq!(before, after, "compaction changed vector search results");
    assert_eq!(db.blob_table.count_rows(None).await.unwrap(), INDEXED_ROWS);
}

/// Compaction must go through the handles the request path already holds, or
/// readers stay pinned to a manifest whose files the prune deleted.
#[tokio::test]
async fn test_reads_through_the_shared_handle_see_the_compacted_dataset() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir).await;
    let records: Vec<BlobRecord> = (0..12).map(blob).collect();
    for r in &records {
        db.insert(r).await.unwrap();
    }

    maintenance::run(&db, true, 1_048_576).await.expect("apply run");

    // Same DbClient, same Table handles that were open before the rewrite.
    let (total, listed) = db.list(None, &[], false, 100, 0).await.unwrap();
    assert_eq!(total, records.len(), "list through the pre-compaction handle lost rows");
    assert_eq!(listed.len(), records.len());
    for r in &records {
        assert!(db.get_by_id(r.id).await.is_ok(), "{} unreadable after compaction", r.name);
    }
}
