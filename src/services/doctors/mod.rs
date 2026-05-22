//! Data-integrity doctors for trips, loads, and facilities.
//!
//! Each `*_doctor` runs a series of deterministic checks against a single
//! resource and produces a structured [`DoctorReport`]. Findings carry an
//! optional [`ProposedFix`]; when `apply == true` the doctor runs each fix
//! that is `safe_to_auto_apply` (no conflicts with existing non-null values)
//! and records the result in `applied` / `skipped_due_to_conflict` /
//! `unfixable`.
//!
//! Design rules:
//!
//! - **Dry-run by default.** The MCP and HTTP wrappers default `apply` to
//!   `false`. Callers see the diagnosis before mutating anything.
//! - **Diff-and-confirm semantics.** Fill-from-load fixes only populate
//!   `None` fields on the target; any non-null field that disagrees with the
//!   source is reported as a conflict and the fix is *not* applied. A human
//!   or higher-level agent must decide.
//! - **No cross-resource cascade.** trip_doctor does not silently call
//!   load_doctor or facility_doctor. It *reports* "facility X failed checks
//!   — recommend facility_doctor"; the caller composes.
//! - **Surgical primitives only.** Doctors apply fixes by calling existing
//!   db / service helpers; they don't reach into LanceDB or invent new write
//!   paths.

use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

pub mod facility;
pub mod load;
pub mod trip;

#[derive(Debug, Clone, Copy, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ProposedFix {
    /// Short identifier of the fix action, e.g. `"resync_stops_from_load"`.
    pub kind: String,
    /// Human-readable description of what the fix would do.
    pub description: String,
    /// Existing non-null target fields that would be touched by the fix.
    /// Non-empty means the fix is **not** applied automatically; the caller
    /// must reconcile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
    /// `true` iff this fix has no conflicts AND is deterministic enough to
    /// apply without a human confirming the change.
    pub safe_to_auto_apply: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Finding {
    /// Stable short id of the check, e.g. `"trip.stops.scheduled_arrive_present"`.
    pub check: String,
    pub severity: Severity,
    /// Human-readable description of the finding.
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<ProposedFix>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DoctorReport {
    pub resource_type: String,
    pub resource_id: Uuid,
    pub dry_run: bool,
    pub findings: Vec<Finding>,
    /// Check ids whose fixes were applied this run (empty when `dry_run`).
    #[serde(default)]
    pub applied: Vec<String>,
    /// Check ids whose fixes were skipped because the proposed fix had
    /// conflicts with existing non-null fields.
    #[serde(default)]
    pub skipped_due_to_conflict: Vec<String>,
    /// Check ids for findings that have no auto-fix (the caller must handle).
    #[serde(default)]
    pub unfixable: Vec<String>,
}

impl DoctorReport {
    pub fn new(resource_type: &str, resource_id: Uuid, dry_run: bool) -> Self {
        Self {
            resource_type: resource_type.to_string(),
            resource_id,
            dry_run,
            findings: Vec::new(),
            applied: Vec::new(),
            skipped_due_to_conflict: Vec::new(),
            unfixable: Vec::new(),
        }
    }

    pub fn push(&mut self, f: Finding) {
        self.findings.push(f);
    }

    /// Buckets each finding's fix-status into `applied` /
    /// `skipped_due_to_conflict` / `unfixable`. Call after all auto-apply
    /// passes complete.
    pub fn classify_findings(&mut self) {
        for f in &self.findings {
            match &f.fix {
                None => {
                    if !self.unfixable.contains(&f.check) {
                        self.unfixable.push(f.check.clone());
                    }
                }
                Some(fix) if !fix.conflicts.is_empty() => {
                    if !self.skipped_due_to_conflict.contains(&f.check) {
                        self.skipped_due_to_conflict.push(f.check.clone());
                    }
                }
                Some(_) => {
                    // Either applied (already recorded by the doctor) or
                    // would-be-applied in non-dry-run; nothing to do here.
                }
            }
        }
    }
}
