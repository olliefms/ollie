//! `facility_doctor` — integrity checks for a single facility.
//!
//! Checks:
//! - `facility.address_present`        — non-empty address.
//! - `facility.coords_present`         — has lat/lng.
//! - `facility.coords_in_us_bbox`      — lat/lng within continental-US bounding box (sanity).
//! - `facility.normalized_address`     — geocoded facilities have a normalized address.
//!
//! Auto-fixes (apply=true):
//! - `facility.geocode_retry` — when geocode_status=PermanentlyFailed AND the
//!   address is non-empty AND no manual coords have been set, reset failure
//!   state to Pending and push to the geocoding worker. Never overwrites
//!   existing data. Setting manual coordinates remains a deliberate dispatcher
//!   action via `update_facility` — the doctor does not invent coordinates.

use uuid::Uuid;

use crate::{
    error::AppError,
    models::{FacilityRecord, GeocodeStatus},
    AppState,
};

use super::{DoctorReport, Finding, ProposedFix, Severity};

// Continental-US bounding box (deliberately loose to allow AK/HI outliers as
// warnings rather than hard errors). Tightening this would require a per-
// region config the dispatcher doesn't have today.
const US_LAT_MIN: f64 = 24.0;
const US_LAT_MAX: f64 = 50.0;
const US_LNG_MIN: f64 = -125.0;
const US_LNG_MAX: f64 = -66.0;

pub async fn run(state: &AppState, facility_id: Uuid, apply: bool) -> Result<DoctorReport, AppError> {
    let fac = state.db.get_facility_by_id(facility_id).await?;
    let mut report = DoctorReport::new("facility", facility_id, !apply);

    check_address_present(&fac, &mut report);
    check_coords_present(&fac, &mut report);
    check_coords_in_us_bbox(&fac, &mut report);
    check_normalized_address(&fac, &mut report);
    check_geocode_retry(&fac, &mut report);

    if apply {
        apply_safe_fixes(state, facility_id, &mut report).await?;
    }
    report.classify_findings();
    Ok(report)
}

fn check_address_present(fac: &FacilityRecord, report: &mut DoctorReport) {
    if fac.address.trim().is_empty() {
        report.push(Finding {
            check: "facility.address_present".into(),
            severity: Severity::Error,
            description: "facility has no address — geocoder and search cannot resolve it".into(),
            fix: None,
        });
    }
}

fn check_coords_present(fac: &FacilityRecord, report: &mut DoctorReport) {
    if fac.lat.is_some() && fac.lng.is_some() { return; }
    let status = format!("{:?}", fac.geocode_status);
    let hint = match fac.geocode_status {
        GeocodeStatus::Failed => "geocoder rejected the address; consider setting manual coordinates via PATCH /api/v1/facilities/:id with lat+lng",
        GeocodeStatus::Pending => "geocode is still pending; check the geocoding worker",
        _ => "facility has no coordinates",
    };
    report.push(Finding {
        check: "facility.coords_present".into(),
        severity: Severity::Error,
        description: format!("no lat/lng on facility (geocode_status={status}). {hint}"),
        fix: None,
    });
}

fn check_coords_in_us_bbox(fac: &FacilityRecord, report: &mut DoctorReport) {
    let (Some(lat), Some(lng)) = (fac.lat, fac.lng) else { return; };
    if !lat.is_finite() || !lng.is_finite() {
        report.push(Finding {
            check: "facility.coords_in_us_bbox".into(),
            severity: Severity::Error,
            description: format!("coordinates non-finite: lat={lat}, lng={lng}"),
            fix: None,
        });
        return;
    }
    if !(US_LAT_MIN..=US_LAT_MAX).contains(&lat) || !(US_LNG_MIN..=US_LNG_MAX).contains(&lng) {
        report.push(Finding {
            check: "facility.coords_in_us_bbox".into(),
            severity: Severity::Warning,
            description: format!(
                "coordinates ({lat:.4}, {lng:.4}) fall outside the continental US \
                 bounding box. Could be legitimate (AK/HI/PR) or a swapped lat/lng."
            ),
            fix: None,
        });
    }
}

/// Surface a re-queue auto-fix when a facility is stuck at `PermanentlyFailed`
/// with no manual coords. The fix only resets state — it never invents coords.
fn check_geocode_retry(fac: &FacilityRecord, report: &mut DoctorReport) {
    if !matches!(fac.geocode_status, GeocodeStatus::PermanentlyFailed) { return; }
    if fac.address.trim().is_empty() { return; }
    if fac.lat.is_some() || fac.lng.is_some() { return; }

    report.push(Finding {
        check: "facility.geocode_retry".into(),
        severity: Severity::Warning,
        description: format!(
            "geocode_status=permanently_failed after {} failures with no manual \
             coords set. The address may have started resolving since the last \
             attempt — apply=true will re-queue it.",
            fac.geocode_failure_count,
        ),
        fix: Some(ProposedFix {
            kind: "geocode_retry".into(),
            description: "Reset geocode_status to pending, clear the failure \
                          count, and push the facility back onto the geocoding \
                          worker. Coordinates and address are left untouched."
                .into(),
            conflicts: Vec::new(),
            safe_to_auto_apply: true,
        }),
    });
}

async fn apply_safe_fixes(
    state: &AppState,
    facility_id: Uuid,
    report: &mut DoctorReport,
) -> Result<(), AppError> {
    let to_apply: Vec<String> = report.findings.iter()
        .filter_map(|f| match &f.fix {
            Some(fix) if fix.safe_to_auto_apply => Some(f.check.clone()),
            _ => None,
        })
        .collect();

    for check_id in to_apply {
        match check_id.as_str() {
            "facility.geocode_retry" => {
                state.db.retry_facility_geocode(facility_id).await?;
                let _ = state.geocoding_tx.try_send(facility_id);
                report.applied.push(check_id);
            }
            _ => {
                tracing::warn!("facility_doctor: no applier wired for check {check_id}");
            }
        }
    }
    Ok(())
}

fn check_normalized_address(fac: &FacilityRecord, report: &mut DoctorReport) {
    if matches!(fac.geocode_status, GeocodeStatus::Ready) && fac.normalized_address.is_none() {
        report.push(Finding {
            check: "facility.normalized_address".into(),
            severity: Severity::Info,
            description: "geocode_status=ready but normalized_address is null — \
                          likely a manually-set facility. Search results will fall \
                          back to the raw address.".into(),
            fix: None,
        });
    }
}
