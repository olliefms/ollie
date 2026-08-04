//! `load_doctor` — integrity checks for a single load.
//!
//! Checks:
//! - `load.stops.facility_geocoded`        — each stop's facility has lat/lng.
//! - `load.stops.scheduled_window_valid`   — `scheduled_arrive_end >= scheduled_arrive`.
//! - `load.stops.actual_order_valid`       — `actual_depart > actual_arrive` when both set.
//! - `load.stops.timezone_present`         — timezone set wherever actual or scheduled times are present.
//! - `load.rate_items.sum_matches_total`   — rate_items sum within 0.01 of `total_rate_usd()`.
//! - `load.status_matches_trips`           — load still pre-delivery while every live trip has delivered.

use uuid::Uuid;

use crate::{
    error::AppError,
    models::{load_trips_all_delivered, LoadRecord, LoadStatus, StopType},
    AppState,
};

use super::{DoctorReport, Finding, ProposedFix, Severity};

pub async fn run(state: &AppState, load_id: Uuid, apply: bool) -> Result<DoctorReport, AppError> {
    let load = state.db.get_load_by_id(load_id).await?;
    let mut report = DoctorReport::new("load", load_id, !apply);

    check_facilities_geocoded(state, &load, &mut report).await;
    check_scheduled_windows(&load, &mut report);
    check_actual_order(&load, &mut report);
    check_timezones(&load, &mut report);
    check_rate_sum(&load, &mut report);
    check_status_matches_trips(state, &load, &mut report).await;

    if apply {
        apply_safe_fixes(state, &load, &mut report).await?;
    }

    report.classify_findings();
    Ok(report)
}

async fn check_facilities_geocoded(state: &AppState, load: &LoadRecord, report: &mut DoctorReport) {
    let ids: Vec<Uuid> = load.stops.iter().map(|s| s.facility_id).collect();
    if ids.is_empty() { return; }
    let facs = state.db.batch_get_facilities(&ids).await.unwrap_or_default();
    let mut ungeocoded: Vec<(u32, Uuid, String)> = Vec::new();
    for s in &load.stops {
        match facs.get(&s.facility_id) {
            None => ungeocoded.push((s.sequence, s.facility_id, "facility not found".into())),
            Some(f) if f.lat.is_none() || f.lng.is_none() => {
                ungeocoded.push((s.sequence, s.facility_id, format!("status={:?}", f.geocode_status)));
            }
            _ => {}
        }
    }
    if ungeocoded.is_empty() { return; }
    let descs: Vec<String> = ungeocoded.iter()
        .map(|(seq, id, status)| format!("stop[{seq}] facility {id} ({status})"))
        .collect();
    report.push(Finding {
        check: "load.stops.facility_geocoded".into(),
        severity: Severity::Warning,
        description: format!(
            "{} stop facilit(ies) are not geocoded — routing will fail. \
             Run facility_doctor on each, or set coordinates manually: {}",
            ungeocoded.len(), descs.join("; "),
        ),
        fix: None, // delegated to facility_doctor
    });
}

fn check_scheduled_windows(load: &LoadRecord, report: &mut DoctorReport) {
    for s in &load.stops {
        let Some(end) = &s.scheduled_arrive_end else { continue; };
        if end < &s.scheduled_arrive {
            report.push(Finding {
                check: "load.stops.scheduled_window_valid".into(),
                severity: Severity::Error,
                description: format!(
                    "stop[{}] has scheduled_arrive_end ({end}) before scheduled_arrive ({}). \
                     The window is malformed; the fleet_user likely flipped open/close times.",
                    s.sequence, s.scheduled_arrive,
                ),
                fix: None,
            });
        }
    }
}

fn check_actual_order(load: &LoadRecord, report: &mut DoctorReport) {
    for s in &load.stops {
        let (Some(a), Some(d)) = (&s.actual_arrive, &s.actual_depart) else { continue; };
        if d <= a {
            report.push(Finding {
                check: "load.stops.actual_order_valid".into(),
                severity: Severity::Error,
                description: format!(
                    "stop[{}] actual_depart ({d}) is not after actual_arrive ({a}). \
                     Driver/fleet_user likely transposed the two when recording.",
                    s.sequence,
                ),
                fix: None,
            });
        }
    }
}

fn check_timezones(load: &LoadRecord, report: &mut DoctorReport) {
    for s in &load.stops {
        let has_actuals = s.actual_arrive.is_some() || s.actual_depart.is_some();
        if has_actuals && s.timezone.is_none() {
            report.push(Finding {
                check: "load.stops.timezone_present".into(),
                severity: Severity::Warning,
                description: format!(
                    "stop[{}] has actual_arrive/depart but no timezone — UTC \
                     conversion cannot be derived for response builders.",
                    s.sequence,
                ),
                fix: None,
            });
        }
    }
}

/// The load's status is denormalized from its trips. When every live trip has
/// delivered but the load is still sitting in a pre-delivery status, the
/// delivery cascade never fired (or was rejected) and the load is stranded —
/// `invoice` and `settle` both refuse from anything but `delivered`, so there
/// is no supported way forward without this fix (#395).
async fn check_status_matches_trips(state: &AppState, load: &LoadRecord, report: &mut DoctorReport) {
    if !matches!(load.status, LoadStatus::Dispatched | LoadStatus::InTransit) {
        return;
    }
    let Ok(trips) = state.db.list_trips_for_load(load.id).await else { return };
    if !load_trips_all_delivered(&trips) {
        return;
    }
    let summary: Vec<String> = trips
        .iter()
        .map(|t| format!("{} ({})", t.trip_number, t.status.as_str()))
        .collect();
    let conflicts = unserved_delivery_stops(load);
    let safe_to_auto_apply = conflicts.is_empty();
    report.push(Finding {
        check: "load.status_matches_trips".into(),
        severity: Severity::Error,
        description: format!(
            "load is '{}' but every live trip has delivered: {}. The load is stranded — \
             invoice and settle both require 'delivered'.",
            load.status.as_str(),
            summary.join(", "),
        ),
        fix: Some(ProposedFix {
            kind: "advance_load_to_delivered".into(),
            description: format!(
                "transition the load '{}' -> 'delivered', the cascade the trips already earned",
                load.status.as_str(),
            ),
            conflicts,
            safe_to_auto_apply,
        }),
    });
}

/// Delivery stops the load's own record says were never served, as
/// `ProposedFix` conflicts.
///
/// "Every live trip has delivered" is necessary but not sufficient: cancelling
/// the one *planned* trip that covered the back half of a relay also leaves the
/// load with nothing but delivered trips, and advancing it would claim freight
/// reached a stop nobody visited. `Delivered` has no reverse edge
/// (`can_transition_to` only allows `Delivered -> Invoiced`), so a wrong
/// auto-apply is as unrecoverable as the strand it was meant to fix.
///
/// The trip cascade writes `actual_depart` through to the linked load stop, so
/// a load carrying actuals on some stops but not its deliveries is actively
/// contradicting the trips, and the fix is held for a human. A load with *no*
/// stop actuals at all is a different case: `load_stop_index` is only populated
/// when a trip derives its stops from the load (`apply_trip_create` in
/// `src/api/trips.rs`), so a trip created with explicit stops never cascades
/// actuals down and the load has nothing to say either way. Treating that
/// silence as a conflict would make the repair path inert for most real loads,
/// so absence of signal is not evidence of an unserved stop.
fn unserved_delivery_stops(load: &LoadRecord) -> Vec<String> {
    let carries_actuals = load.stops.iter()
        .any(|s| s.actual_arrive.is_some() || s.actual_depart.is_some());
    if !carries_actuals {
        return Vec::new();
    }
    load.stops.iter()
        .filter(|s| s.stop_type == StopType::Delivery && s.actual_depart.is_none())
        .map(|s| format!("stop[{}] has no actual_depart — it was never served", s.sequence))
        .collect()
}

fn check_rate_sum(load: &LoadRecord, report: &mut DoctorReport) {
    let sum: f64 = load.rate_items.iter().map(|r| r.amount_usd).sum();
    let total = load.total_rate_usd();
    if (sum - total).abs() > 0.01 {
        report.push(Finding {
            check: "load.rate_items.sum_matches_total".into(),
            severity: Severity::Warning,
            description: format!(
                "rate_items sum ${sum:.2} differs from total_rate_usd() ${total:.2} \
                 by more than 1¢. Likely a duplicate line item or stale snapshot."
            ),
            fix: None,
        });
    }
}

// ---------------------------------------------------------------------------
// Auto-apply
// ---------------------------------------------------------------------------

async fn apply_safe_fixes(
    state: &AppState, load: &LoadRecord, report: &mut DoctorReport,
) -> Result<(), AppError> {
    let to_apply: Vec<String> = report.findings.iter()
        .filter_map(|f| match &f.fix {
            Some(fix) if fix.safe_to_auto_apply => Some(f.check.clone()),
            _ => None,
        })
        .collect();

    for check_id in to_apply {
        match check_id.as_str() {
            // A failure here surfaces to the caller rather than being logged and
            // dropped: `apply=true` returning a report that looks like a success
            // is the same silent-failure shape #395 was about.
            "load.status_matches_trips" => {
                state.db
                    .transition_load_status(load.id, LoadStatus::Delivered, None, None, None)
                    .await?;
                report.applied.push(check_id);
            }
            _ => {
                // Defensive: a finding marked safe_to_auto_apply has no wired-up
                // applier. Never reached in a well-formed build.
                tracing::warn!("load_doctor: no applier wired for check {check_id}");
            }
        }
    }

    Ok(())
}
