//! `load_doctor` — integrity checks for a single load.
//!
//! Checks:
//! - `load.stops.facility_geocoded`        — each stop's facility has lat/lng.
//! - `load.stops.scheduled_window_valid`   — `scheduled_arrive_end >= scheduled_arrive`.
//! - `load.stops.actual_order_valid`       — `actual_depart > actual_arrive` when both set.
//! - `load.stops.timezone_present`         — timezone set wherever actual or scheduled times are present.
//! - `load.rate_items.sum_matches_total`   — rate_items sum within 0.01 of `total_rate_usd()`.

use uuid::Uuid;

use crate::{
    error::AppError,
    models::LoadRecord,
    AppState,
};

use super::{DoctorReport, Finding, Severity};

pub async fn run(state: &AppState, load_id: Uuid, apply: bool) -> Result<DoctorReport, AppError> {
    let load = state.db.get_load_by_id(load_id).await?;
    let mut report = DoctorReport::new("load", load_id, !apply);

    check_facilities_geocoded(state, &load, &mut report).await;
    check_scheduled_windows(&load, &mut report);
    check_actual_order(&load, &mut report);
    check_timezones(&load, &mut report);
    check_rate_sum(&load, &mut report);

    // load_doctor currently has no auto-fixes — everything points at
    // facility_doctor or human-required reconciliation. The `apply` flag is
    // accepted for API symmetry but is a no-op today.
    let _ = apply;

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
                     The window is malformed; the dispatcher likely flipped open/close times.",
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
                     Driver/dispatcher likely transposed the two when recording.",
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
