//! `facility_doctor` — integrity checks for a single facility.
//!
//! Checks:
//! - `facility.address_present`        — non-empty address.
//! - `facility.coords_present`         — has lat/lng.
//! - `facility.coords_in_us_bbox`      — lat/lng within continental-US bounding box (sanity).
//! - `facility.normalized_address`     — geocoded facilities have a normalized address.
//!
//! No auto-fixes. The two surgical primitives that *would* fix a finding —
//! re-queueing geocode (already wired via address change) and setting
//! manual coordinates (admin endpoint exists) — both require values the
//! doctor cannot derive on its own. Callers see the diagnosis and route
//! to the right primitive.

use uuid::Uuid;

use crate::{
    error::AppError,
    models::{FacilityRecord, GeocodeStatus},
    AppState,
};

use super::{DoctorReport, Finding, Severity};

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

    let _ = apply; // no auto-fixes today
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
