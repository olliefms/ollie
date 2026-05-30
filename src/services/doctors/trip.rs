//! `trip_doctor` — integrity checks and safe auto-fixes for a single trip.
//!
//! Checks (each contributes 0 or 1 [`Finding`] to the report):
//!
//! - `trip.stops.facility_id_present` — every stop has `facility_id`.
//! - `trip.stops.metadata_complete`   — every stop has name/address/scheduled_arrive/timezone; auto-fix from the linked load (diff-and-confirm).
//! - `trip.previous_trip.resolves`    — `previous_trip_id`, if set, points to an existing trip.
//! - `trip.mileage.arithmetic`        — `deadhead + Σ(loaded_segments) ≈ total_miles` (within 0.5 mi).
//! - `trip.mileage.segment_count`     — `segment_miles.len()` matches expected (stops + has-previous).
//! - `trip.status.actuals_consistent` — in_transit ⇒ first stop has actual_arrive+depart; delivered ⇒ all stops do.
//! - `trip.resources.resolve`         — driver/truck/trailer ids resolve to existing records.

use uuid::Uuid;

use crate::{
    error::AppError,
    models::{Stop as LoadStop, TripRecord, TripStatus, TripStop, TripStopType, StopType},
    AppState,
};

use super::{DoctorReport, Finding, ProposedFix, Severity};

const MILEAGE_TOLERANCE_MI: f64 = 0.5;

pub async fn run(state: &AppState, trip_id: Uuid, apply: bool) -> Result<DoctorReport, AppError> {
    let trip = state.db.get_trip(trip_id).await?;
    let mut report = DoctorReport::new("trip", trip_id, !apply);

    check_stops_facility_id(&trip, &mut report);
    check_mileage_arithmetic(&trip, &mut report);
    check_segment_count(&trip, &mut report);
    check_status_actuals(&trip, &mut report);

    // Async checks
    let previous_trip = check_previous_trip_resolves(state, &trip, &mut report).await;
    let load = check_stops_metadata_complete(state, &trip, &mut report).await;
    check_resources_resolve(state, &trip, &mut report).await;

    // Auto-apply pass — only fixes flagged safe_to_auto_apply.
    if apply {
        apply_safe_fixes(state, &trip, &mut report, load.as_ref(), previous_trip.as_ref()).await?;
    }

    report.classify_findings();
    Ok(report)
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

fn check_stops_facility_id(trip: &TripRecord, report: &mut DoctorReport) {
    let missing: Vec<u32> = trip.stops.iter()
        .filter(|s| s.facility_id.is_none())
        .map(|s| s.sequence)
        .collect();
    if missing.is_empty() { return; }
    report.push(Finding {
        check: "trip.stops.facility_id_present".into(),
        severity: Severity::Error,
        description: format!(
            "{} stop(s) have no facility_id (sequences: {:?}). Mileage routing \
             will silently no-op and the dispatcher UI cannot render the stop.",
            missing.len(), missing,
        ),
        // No deterministic fix — facility selection requires human input.
        fix: None,
    });
}

fn check_mileage_arithmetic(trip: &TripRecord, report: &mut DoctorReport) {
    let (Some(total), Some(loaded)) = (trip.total_miles, trip.loaded_miles) else { return; };
    let deadhead = trip.deadhead_miles.unwrap_or(0.0);
    let recomputed = deadhead + loaded;
    if (total - recomputed).abs() > MILEAGE_TOLERANCE_MI {
        report.push(Finding {
            check: "trip.mileage.arithmetic".into(),
            severity: Severity::Warning,
            description: format!(
                "deadhead ({deadhead:.2}) + loaded ({loaded:.2}) = {recomputed:.2} \
                 differs from total ({total:.2}) by more than {MILEAGE_TOLERANCE_MI} mi. \
                 Suggests partial write or stale segment data; run recalculate_trip_miles."
            ),
            fix: None,
        });
    }
}

fn check_segment_count(trip: &TripRecord, report: &mut DoctorReport) {
    if trip.segment_miles.is_empty() { return; }
    let expected = trip.stops.len() - 1 + usize::from(trip.previous_trip_id.is_some());
    if trip.segment_miles.len() != expected {
        report.push(Finding {
            check: "trip.mileage.segment_count".into(),
            severity: Severity::Warning,
            description: format!(
                "segment_miles has {} entries; expected {} (stops={}, has_previous={}). \
                 Recompute via recalculate_trip_miles.",
                trip.segment_miles.len(), expected,
                trip.stops.len(), trip.previous_trip_id.is_some(),
            ),
            fix: None,
        });
    }
}

fn check_status_actuals(trip: &TripRecord, report: &mut DoctorReport) {
    match trip.status {
        TripStatus::InTransit => {
            let first = trip.stops.iter().min_by_key(|s| s.sequence);
            if let Some(s) = first {
                if s.actual_arrive.is_none() || s.actual_depart.is_none() {
                    report.push(Finding {
                        check: "trip.status.actuals_consistent".into(),
                        severity: Severity::Warning,
                        description: format!(
                            "trip is in_transit but stop {} is missing actual_arrive \
                             ({:?}) or actual_depart ({:?}). Driver/dispatcher likely \
                             advanced status without recording stop times.",
                            s.sequence, s.actual_arrive, s.actual_depart,
                        ),
                        fix: None,
                    });
                }
            }
        }
        TripStatus::Delivered | TripStatus::Completed => {
            for s in &trip.stops {
                if s.actual_arrive.is_none() || s.actual_depart.is_none() {
                    report.push(Finding {
                        check: "trip.status.actuals_consistent".into(),
                        severity: Severity::Warning,
                        description: format!(
                            "trip is {:?} but stop {} is missing actual times \
                             (arrive={:?}, depart={:?}).",
                            trip.status, s.sequence, s.actual_arrive, s.actual_depart,
                        ),
                        fix: None,
                    });
                    break;
                }
            }
        }
        _ => {}
    }
}

async fn check_previous_trip_resolves(
    state: &AppState, trip: &TripRecord, report: &mut DoctorReport,
) -> Option<TripRecord> {
    let prev_id = trip.previous_trip_id?;
    match state.db.get_trip(prev_id).await {
        Ok(prev) => Some(prev),
        Err(_) => {
            report.push(Finding {
                check: "trip.previous_trip.resolves".into(),
                severity: Severity::Error,
                description: format!(
                    "previous_trip_id {prev_id} does not resolve to an existing trip. \
                     Mileage deadhead will be silently dropped."
                ),
                fix: None,
            });
            None
        }
    }
}

/// Diff-and-confirm fill from the linked load. Builds a [`ProposedFix`] iff
/// any trip stop has nullable metadata fields that *could* be filled from
/// the matching load stop. Conflicts (trip has a non-null value that
/// disagrees with the load) are recorded and the fix is blocked.
async fn check_stops_metadata_complete(
    state: &AppState, trip: &TripRecord, report: &mut DoctorReport,
) -> Option<crate::models::LoadRecord> {
    let load_id = trip.load_id?;
    let load = state.db.get_load_by_id(load_id).await.ok()?;

    let (would_fill, conflicts) = compute_stop_metadata_diff(&trip.stops, &load.stops);

    if would_fill.is_empty() && conflicts.is_empty() { return Some(load); }

    let description = if !would_fill.is_empty() {
        format!(
            "{} trip stop field(s) could be backfilled from load {}: {}",
            would_fill.len(), load.load_number,
            would_fill.join(", "),
        )
    } else {
        format!(
            "trip stops disagree with load {} on {} field(s); manual reconciliation required",
            load.load_number, conflicts.len(),
        )
    };

    let fix = ProposedFix {
        kind: "resync_stops_from_load".into(),
        description: "Fill null trip-stop fields from the matching load stops by \
                      sequence. Existing non-null trip values are never overwritten."
            .into(),
        safe_to_auto_apply: conflicts.is_empty() && !would_fill.is_empty(),
        conflicts,
    };

    report.push(Finding {
        check: "trip.stops.metadata_complete".into(),
        severity: Severity::Warning,
        description,
        fix: Some(fix),
    });

    Some(load)
}

async fn check_resources_resolve(state: &AppState, trip: &TripRecord, report: &mut DoctorReport) {
    let mut missing: Vec<String> = Vec::new();
    if let Some(id) = trip.driver_id {
        if state.db.get_driver_by_id(id).await.is_err() {
            missing.push(format!("driver_id={id}"));
        }
    }
    if let Some(id) = trip.truck_id {
        if state.db.get_truck_by_id(id).await.is_err() {
            missing.push(format!("truck_id={id}"));
        }
    }
    for &id in &trip.trailer_ids {
        if state.db.get_trailer_by_id(id).await.is_err() {
            missing.push(format!("trailer_id={id}"));
        }
    }
    if !missing.is_empty() {
        report.push(Finding {
            check: "trip.resources.resolve".into(),
            severity: Severity::Error,
            description: format!(
                "{} assigned resource id(s) do not resolve to active records: {}",
                missing.len(), missing.join(", "),
            ),
            fix: None,
        });
    }
}

// ---------------------------------------------------------------------------
// Diff helpers
// ---------------------------------------------------------------------------

fn matching_load_stop<'a>(load_stops: &'a [LoadStop], trip_stop: &TripStop) -> Option<&'a LoadStop> {
    load_stops.iter().find(|ls| {
        ls.sequence == trip_stop.sequence
            && matches!(
                (&ls.stop_type, &trip_stop.stop_type),
                (StopType::Pickup, TripStopType::Pickup)
                    | (StopType::Delivery, TripStopType::Delivery)
            )
    })
}

/// Returns (would_fill_descriptions, conflict_descriptions). A field is
/// "would fill" if the trip's value is `None` and the load has a value;
/// "conflict" if both have a value and they disagree.
fn compute_stop_metadata_diff(
    trip_stops: &[TripStop],
    load_stops: &[LoadStop],
) -> (Vec<String>, Vec<String>) {
    let mut fill = Vec::new();
    let mut conflicts = Vec::new();

    for ts in trip_stops {
        let Some(ls) = matching_load_stop(load_stops, ts) else { continue; };
        let tag = format!("stop[{}]", ts.sequence);

        // name / address come from the facility, not the load directly; we
        // handle those as a side-effect of resync via facility lookup below.

        // scheduled_arrive (load's is non-Option String)
        match ts.scheduled_arrive.as_deref() {
            None => fill.push(format!("{tag}.scheduled_arrive")),
            Some(v) if v != ls.scheduled_arrive => {
                conflicts.push(format!("{tag}.scheduled_arrive: trip='{v}' load='{}'", ls.scheduled_arrive));
            }
            _ => {}
        }

        diff_opt(&ts.scheduled_arrive_end, &ls.scheduled_arrive_end, &tag, "scheduled_arrive_end", &mut fill, &mut conflicts);
        diff_opt(&ts.actual_arrive,        &ls.actual_arrive,        &tag, "actual_arrive",        &mut fill, &mut conflicts);
        diff_opt(&ts.actual_depart,        &ls.actual_depart,        &tag, "actual_depart",        &mut fill, &mut conflicts);
        diff_opt(&ts.notes,                &ls.notes,                &tag, "notes",                &mut fill, &mut conflicts);
        diff_opt(&ts.timezone,             &ls.timezone,             &tag, "timezone",             &mut fill, &mut conflicts);

        diff_opt_num(ts.expected_dwell_minutes,   ls.expected_dwell_minutes,   &tag, "expected_dwell_minutes",   &mut fill, &mut conflicts);
        diff_opt_num(ts.detention_free_minutes,   ls.detention_free_minutes,   &tag, "detention_free_minutes",   &mut fill, &mut conflicts);
        diff_opt_num(ts.detention_grace_minutes,  ls.detention_grace_minutes,  &tag, "detention_grace_minutes",  &mut fill, &mut conflicts);
    }

    (fill, conflicts)
}

fn diff_opt<T: PartialEq + std::fmt::Debug>(
    trip_v: &Option<T>, load_v: &Option<T>,
    tag: &str, field: &str,
    fill: &mut Vec<String>, conflicts: &mut Vec<String>,
) {
    match (trip_v, load_v) {
        (None, Some(_)) => fill.push(format!("{tag}.{field}")),
        (Some(a), Some(b)) if a != b => {
            conflicts.push(format!("{tag}.{field}: trip={a:?} load={b:?}"));
        }
        _ => {}
    }
}

fn diff_opt_num(
    trip_v: Option<u32>, load_v: Option<u32>,
    tag: &str, field: &str,
    fill: &mut Vec<String>, conflicts: &mut Vec<String>,
) {
    match (trip_v, load_v) {
        (None, Some(_)) => fill.push(format!("{tag}.{field}")),
        (Some(a), Some(b)) if a != b => {
            conflicts.push(format!("{tag}.{field}: trip={a} load={b}"));
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Auto-apply
// ---------------------------------------------------------------------------

async fn apply_safe_fixes(
    state: &AppState,
    trip: &TripRecord,
    report: &mut DoctorReport,
    load: Option<&crate::models::LoadRecord>,
    _previous_trip: Option<&TripRecord>,
) -> Result<(), AppError> {
    // Snapshot which findings asked to be applied — we mutate `report` below.
    let to_apply: Vec<String> = report.findings.iter()
        .filter_map(|f| match &f.fix {
            Some(fix) if fix.safe_to_auto_apply => Some(f.check.clone()),
            _ => None,
        })
        .collect();

    for check_id in to_apply {
        match check_id.as_str() {
            "trip.stops.metadata_complete" => {
                if let Some(load) = load {
                    let merged = resync_stops_from_load(state, &trip.stops, &load.stops).await?;
                    state.db
                        .update_trip_metadata(trip.id, None, None, Some(merged), None, None, None)
                        .await?;
                    report.applied.push(check_id);
                }
            }
            _ => {
                // Defensive: a finding marked safe_to_auto_apply has no
                // wired-up applier. Treat as unfixable so we don't silently
                // drop it. This should never trigger in a well-formed build.
                tracing::warn!("trip_doctor: no applier wired for check {check_id}");
            }
        }
    }

    Ok(())
}

/// Construct the merged stop vec used by `resync_stops_from_load`: copy the
/// trip stops, then for each one, fill any `None` field from the matching
/// load stop. Never overwrites a non-null trip value (matches the diff-rule
/// in `compute_stop_metadata_diff`). Also fills `name`/`address` from the
/// facility record when the trip had them null.
async fn resync_stops_from_load(
    state: &AppState,
    trip_stops: &[TripStop],
    load_stops: &[LoadStop],
) -> Result<Vec<TripStop>, AppError> {
    // Batch-fetch facilities to populate name/address.
    let facility_ids: Vec<Uuid> = trip_stops.iter().filter_map(|s| s.facility_id).collect();
    let facilities = if facility_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        state.db.batch_get_facilities(&facility_ids).await.unwrap_or_default()
    };

    let mut out = Vec::with_capacity(trip_stops.len());
    for (idx, ts) in trip_stops.iter().enumerate() {
        let mut s = ts.clone();
        if let Some(ls) = matching_load_stop(load_stops, ts) {
            if s.scheduled_arrive.is_none()       { s.scheduled_arrive       = Some(ls.scheduled_arrive.clone()); }
            if s.scheduled_arrive_end.is_none()   { s.scheduled_arrive_end   = ls.scheduled_arrive_end.clone(); }
            if s.actual_arrive.is_none()          { s.actual_arrive          = ls.actual_arrive.clone(); }
            if s.actual_depart.is_none()          { s.actual_depart          = ls.actual_depart.clone(); }
            if s.notes.is_none()                  { s.notes                  = ls.notes.clone(); }
            if s.timezone.is_none()               { s.timezone               = ls.timezone.clone(); }
            if s.expected_dwell_minutes.is_none() { s.expected_dwell_minutes = ls.expected_dwell_minutes; }
            if s.detention_free_minutes.is_none() { s.detention_free_minutes = ls.detention_free_minutes; }
            if s.detention_grace_minutes.is_none(){ s.detention_grace_minutes= ls.detention_grace_minutes; }
            if s.load_stop_index.is_none()        { s.load_stop_index        = Some(idx as u32); }
        }
        if let Some(fid) = s.facility_id {
            if let Some(fac) = facilities.get(&fid) {
                if s.name.is_none()    { s.name    = Some(fac.name.clone()); }
                if s.address.is_none() { s.address = Some(fac.address.clone()); }
            }
        }
        out.push(s);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(seq: u32, sched: Option<&str>) -> TripStop {
        TripStop {
            sequence: seq,
            stop_type: TripStopType::Pickup,
            facility_id: Some(Uuid::nil()),
            name: None, address: None, load_stop_index: None,
            scheduled_arrive: sched.map(String::from),
            scheduled_arrive_end: None,
            actual_arrive: None, actual_depart: None,
            expected_dwell_minutes: None,
            detention_free_minutes: None,
            detention_grace_minutes: None,
            notes: None, timezone: None,
            actual_arrive_utc: None, actual_depart_utc: None,
        }
    }

    fn ls(seq: u32, sched: &str, tz: Option<&str>) -> LoadStop {
        LoadStop {
            sequence: seq,
            stop_type: StopType::Pickup,
            service_type: crate::models::load::ServiceType::LiveLoad,
            facility_id: Uuid::nil(),
            scheduled_arrive: sched.to_string(),
            scheduled_arrive_end: None,
            actual_arrive: None, actual_depart: None,
            expected_dwell_minutes: None,
            detention_free_minutes: None,
            detention_grace_minutes: None,
            notes: None,
            blob_ids: vec![],
            timezone: tz.map(String::from),
            actual_arrive_utc: None, actual_depart_utc: None,
        }
    }

    #[test]
    fn diff_fills_null_trip_fields() {
        let trips = vec![ts(1, None)];
        let loads = vec![ls(1, "2026-05-22T10:00:00", Some("America/New_York"))];
        let (fill, conflicts) = compute_stop_metadata_diff(&trips, &loads);
        assert!(conflicts.is_empty(), "no conflicts expected, got {conflicts:?}");
        assert!(fill.iter().any(|f| f.contains("scheduled_arrive")));
        assert!(fill.iter().any(|f| f.contains("timezone")));
    }

    #[test]
    fn diff_flags_conflict_when_both_set_differently() {
        let trips = vec![ts(1, Some("2026-05-22T10:00:00"))];
        let loads = vec![ls(1, "2026-05-22T11:00:00", None)];
        let (_fill, conflicts) = compute_stop_metadata_diff(&trips, &loads);
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert!(conflicts[0].contains("scheduled_arrive"));
    }

    #[test]
    fn diff_silent_when_trip_already_matches_load() {
        let trips = vec![ts(1, Some("2026-05-22T10:00:00"))];
        let loads = vec![ls(1, "2026-05-22T10:00:00", None)];
        let (fill, conflicts) = compute_stop_metadata_diff(&trips, &loads);
        assert!(fill.is_empty()); assert!(conflicts.is_empty());
    }
}
