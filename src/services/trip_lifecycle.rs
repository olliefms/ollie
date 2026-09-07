//! Surface-agnostic trip-lifecycle business logic.
//!
//! The Fleet REST API (`/fleet/api/v1`) and the Fleet MCP server both drive
//! the same trip state machine: assign,
//! unassign, dispatch, undispatch, cancel, complete, tonu, plus the
//! late/check-call event emitters. Each surface owns its auth and request-shape concerns; the
//! cascades (resource status, linked-load status), the events, and the re-fetch
//! all live here so every surface behaves identically.

use crate::events;
use crate::models::{DriverStatus, LoadStatus, TrailerStatus, TripRecord, TripStatus, TruckStatus};
use crate::{error::AppError, AppState};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

/// Upper bound on a `previous_trip_id` walk. Chains are a handful of legs; the
/// bound exists so a cycle already in the data cannot hang a request.
const CHAIN_WALK_LIMIT: usize = 64;

/// Walk a load's denormalized status back down after its trips released it.
/// Best-effort like the resource cascades — a stale load status must not fail the
/// trip operation the caller asked for — but the failure is logged rather than
/// discarded: a silently swallowed `Assigned -> Planned` rejection is exactly what
/// left production loads claiming `assigned` with no trip holding them.
async fn demote_released_load(state: &AppState, load_id: Uuid, target: LoadStatus) {
    let label = target.as_str();
    if let Err(e) = state
        .db
        .transition_load_status(load_id, target, None, None, None, None)
        .await
    {
        tracing::warn!(
            %load_id, target = label, error = %e,
            "load status not demoted after its trip was released"
        );
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AssignTripRequest {
    pub driver_id: Uuid,
    pub truck_id: Uuid,
    #[serde(default)]
    pub trailer_ids: Vec<Uuid>,
    /// Which trip this one follows — the auto-dispatch chain link (#437).
    ///
    /// Three-state on purpose: **absent** means "decide for me" and derives from
    /// the driver's current work, **`null`** means "this starts a new chain" and
    /// sets no link, and **a value** pins that trip. Without the middle state
    /// there would be no way to say "no chain" on an assignment at all.
    ///
    /// Note `null` is not durable state: it persists as `None`, which is
    /// indistinguishable from never-derived, so a later unassign/re-assign cycle
    /// with no stated preference will derive a link again. Telling the two apart
    /// would need a column, which this issue does not add.
    ///
    /// Note this field is also the deadhead origin, so setting it recomputes the
    /// trip's mileage, which feeds driver pay.
    #[serde(default, deserialize_with = "crate::models::double_option")]
    pub previous_trip_id: Option<Option<Uuid>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StopArriveRequest {
    pub actual_arrive: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StopDepartRequest {
    pub actual_depart: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StopLateRequest {
    pub eta: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CheckCallRequest {
    pub location: String,
    pub notes: Option<String>,
    pub eta_next_stop: Option<String>,
}

/// A position the dispatcher supplies for a stop that was not in the plan:
/// either an existing facility, or a name + address resolved (or created)
/// through the same path `StopInput` uses.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PositionInput {
    pub facility_id: Option<Uuid>,
    pub facility_name: Option<String>,
    pub address: Option<String>,
    /// IANA timezone, required so the naive local arrive/depart strings parse.
    pub timezone: String,
    pub actual_arrive: Option<String>,
    pub actual_depart: Option<String>,
    pub notes: Option<String>,
    /// Overrides the caller's default. A diversion destination is normally a
    /// `delivery`, but a cross-dock hand-off is a `relay`.
    #[serde(default)]
    pub stop_type: Option<crate::models::TripStopType>,
}

#[derive(Debug, Deserialize, ToSchema, Default)]
pub struct TonuRequest {
    /// Required when no stop was reached; optional (and appended) otherwise.
    #[serde(default)]
    pub waypoint: Option<PositionInput>,
    /// When the driver was released. Naive local in the truncation stop's own
    /// timezone; defaults to now in that zone. Rejected when no stop was reached
    /// — use `waypoint.actual_depart` there instead.
    #[serde(default)]
    pub occurred_at: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DivertReason {
    /// The broker cancelled mid-transit.
    Diverted,
    /// The broker nominated a different consignee.
    Reconsigned,
    /// The BOL disagreed with the rate confirmation and the BOL wins. Nothing
    /// was diverted — the plan was wrong from the start.
    BolCorrection,
}

impl DivertReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Diverted => "diverted",
            Self::Reconsigned => "reconsigned",
            Self::BolCorrection => "bol_correction",
        }
    }
    /// Whether this reason represents a commercial diversion with a fee to
    /// negotiate, and therefore flags the load.
    fn flags_the_load(&self) -> bool { !matches!(self, Self::BolCorrection) }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DivertRequest {
    /// Where the old plan and the new plan diverged. Required, because an
    /// in-transit truck is by definition between the trip's existing points —
    /// EXCEPT when the trip already ends at a waypoint the driver reached, which
    /// is where a previous hold-only divert left it. The stop list already ends
    /// at the truck's real position there, so the new destination is simply
    /// appended and no fresh divergence point exists to name.
    #[serde(default)]
    pub waypoint: Option<PositionInput>,
    /// Replacement for every stop the driver has not reached. May be empty —
    /// "pulled over, disposition unknown".
    #[serde(default)]
    pub stops: Vec<PositionInput>,
    pub reason: DivertReason,
    #[serde(default)]
    pub notes: Option<String>,
}

/// What every trip-outcome verb returns: the trip as it now stands, plus a
/// warning when its mileage could not be recomputed.
///
/// TONU and diversion are both operational facts that must be recordable with
/// ORS down, so a routing failure degrades to this field rather than failing the
/// call. In both cases the miles are left as an honest `null` rather than the
/// figure for a plan that no longer describes the trip.
#[derive(Debug, serde::Serialize, ToSchema)]
pub struct TripOutcomeResult {
    #[serde(flatten)]
    pub trip: TripRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mileage_recompute_warning: Option<String>,
}

/// Validates that a driver/truck/trailers are eligible for assignment WITHOUT
/// mutating anything, returning the fetched records. Callers that create-then-
/// assign (e.g. `apply_trip_create`) run this before any insert so a rejected
/// assignment can't leave an orphaned record behind. `assign` uses it too, so
/// the eligibility rules live in exactly one place.
pub(crate) async fn validate_assignment(
    state: &AppState,
    driver_id: Uuid,
    truck_id: Uuid,
    trailer_ids: &[Uuid],
) -> Result<
    (
        crate::models::driver::DriverRecord,
        crate::models::truck::TruckRecord,
        Vec<crate::models::trailer::TrailerRecord>,
    ),
    AppError,
> {
    // Assignment (`planned -> assigned`) is a planning action: it attaches
    // equipment but does not put the driver on the road. The single-active-
    // dispatch rule ("a driver/truck may only be *dispatched* on one trip at a
    // time") therefore belongs to `dispatch`, not here — enforcing it at assign
    // time blocks pre-staging the next leg of a chained trip while the current
    // leg is still live, which is exactly how dedicated lanes are dispatched.
    // We still reject genuinely unavailable resources (inactive/out-of-service),
    // since those can never be dispatched no matter when the trip runs.
    // `assign` only ever promotes Available -> Assigned, so a still-dispatched
    // resource keeps its Dispatched status and stays bound to its live trip.
    let driver = state.db.get_driver_by_id(driver_id).await?;
    if driver.status == DriverStatus::Inactive {
        return Err(AppError::Conflict(format!("driver {driver_id} is not available for assignment")));
    }

    let truck = state.db.get_truck_by_id(truck_id).await?;
    if matches!(truck.status, TruckStatus::OutOfService | TruckStatus::Inactive) {
        return Err(AppError::Conflict(format!("truck {truck_id} is not available for assignment")));
    }

    let mut trailers = Vec::new();
    for &trailer_id in trailer_ids {
        let trailer = state.db.get_trailer_by_id(trailer_id).await?;
        if matches!(trailer.status, TrailerStatus::OutOfService | TrailerStatus::Inactive) {
            return Err(AppError::Conflict(format!(
                "trailer {trailer_id} is not available for assignment"
            )));
        }
        trailers.push(trailer);
    }

    Ok((driver, truck, trailers))
}

pub async fn assign(
    state: &AppState,
    trip_id: Uuid,
    req: AssignTripRequest,
) -> Result<TripRecord, AppError> {
    // Validate (and fetch) all resources before any mutation to prevent partial state.
    let (driver, truck, trailers) =
        validate_assignment(state, req.driver_id, req.truck_id, &req.trailer_ids).await?;

    // #437 chain-link validation belongs up here with the rest: every rejectable
    // condition must be checked before the first write, or a caller gets a 409
    // with the assignment already half-applied.
    let existing = state.db.get_trip(trip_id).await?;
    if existing.settlement_ref.is_some() && req.previous_trip_id.is_some() {
        // Matches `apply_trip_patch`: repointing this field recomputes mileage,
        // and a settled trip's miles are frozen. Silently dropping the caller's
        // value instead would return 200 for a request that did not happen.
        return Err(AppError::Conflict(
            "trip is settled; previous_trip_id is frozen (it would recompute miles)".into()));
    }
    if let Some(Some(prev_id)) = req.previous_trip_id {
        validate_chain_link(state, trip_id, prev_id, req.driver_id).await?;
    }
    // Only a first assignment derives.
    //
    // `assign` has no status precondition of its own, so calling it on a
    // *Dispatched* trip succeeds by reusing the `Dispatched -> Assigned` edge
    // that exists for `undispatch`. (A trip that is actually rolling is safe:
    // there is no `InTransit -> Assigned` edge, so it fails at the transition.)
    // That second assignment must not re-derive: by then the trip's own successor
    // is the only chainable candidate left, and chaining to it builds a 2-cycle
    // that blocks both trips from ever dispatching and recomputes the deadhead of
    // a trip already released to a driver.
    //
    // This is a guard, not an endorsement — re-assigning a released trip is an
    // artefact of that missing precondition rather than a designed workflow, and
    // the fleet UI only offers Assign on `planned`.
    let derive_allowed = existing.status == TripStatus::Planned;

    state.db.transition_trip_status(trip_id, TripStatus::Assigned).await?;
    state
        .db
        .update_trip_resources(trip_id, Some(req.driver_id), Some(req.truck_id), req.trailer_ids.clone())
        .await?;

    if driver.status == DriverStatus::Available {
        state.db.update_driver_status(req.driver_id, DriverStatus::Assigned).await?;
    }
    if truck.status == TruckStatus::Available {
        state.db.update_truck_status(req.truck_id, TruckStatus::Assigned).await?;
    }
    for trailer in &trailers {
        if trailer.status == TrailerStatus::Available {
            state.db.update_trailer_status(trailer.id, TrailerStatus::Assigned).await?;
        }
    }

    let trip = state.db.get_trip(trip_id).await?;
    let trip = resolve_chain_link(state, trip, &req, derive_allowed).await;

    if let Some(load_id) = trip.load_id {
        if let Ok(load) = state.db.get_load_by_id(load_id).await {
            if load.status == LoadStatus::Planned {
                let _ = state.db.transition_load_status(load_id, LoadStatus::Assigned, None, None, None, None).await;
            }
        }
    }

    events::on_trip_assigned(&state.db, trip_id).await;
    Ok(trip)
}

/// Reject a caller-supplied chain link that could not work. Runs before any
/// write (#437).
///
/// `apply_trip_patch` accepts any UUID here, so a bad link is already reachable;
/// this path is not going to widen that hole, especially now that the
/// `assign_driver` MCP description invites agents to pass ids.
async fn validate_chain_link(
    state: &AppState,
    trip_id: Uuid,
    prev_id: Uuid,
    driver_id: Uuid,
) -> Result<(), AppError> {
    if prev_id == trip_id {
        return Err(AppError::UnprocessableEntity(
            "a trip cannot follow itself".into()));
    }
    let prev = state.db.get_trip(prev_id).await.map_err(|_| {
        // `predecessor_blocking_dispatch` treats an unreadable predecessor as a
        // hard block, so accepting a dangling id would silently strand the trip.
        AppError::UnprocessableEntity(format!("previous trip {prev_id} does not exist"))
    })?;
    // Only a DIFFERENT driver is a problem. A predecessor with no driver is the
    // ordinary chain origin the create path produces — it supplies the deadhead
    // origin for mileage and is legitimate.
    if prev.driver_id.is_some() && prev.driver_id != Some(driver_id) {
        // Auto-dispatch only ever looks at one driver's trips, so a cross-driver
        // link can never fire — it would just block this trip permanently.
        return Err(AppError::UnprocessableEntity(format!(
            "previous trip {prev_id} is assigned to a different driver"
        )));
    }
    if chain_reaches(state, prev_id, trip_id).await {
        return Err(AppError::UnprocessableEntity(format!(
            "previous trip {prev_id} already follows this trip; that would form a cycle"
        )));
    }
    Ok(())
}

/// Whether walking `previous_trip_id` back from `from_id` reaches `target_id`.
/// Bounded so a pre-existing cycle in the data cannot hang the request.
async fn chain_reaches(state: &AppState, from_id: Uuid, target_id: Uuid) -> bool {
    let mut cursor = Some(from_id);
    for _ in 0..CHAIN_WALK_LIMIT {
        let Some(id) = cursor else { return false };
        if id == target_id { return true; }
        cursor = match state.db.get_trip(id).await {
            Ok(t) => t.previous_trip_id,
            Err(_) => return false,
        };
    }
    false
}

/// Settle `previous_trip_id` at assign time (#437), returning the trip as it now
/// stands. Best-effort throughout: the assignment itself has already succeeded,
/// and a chain link is not worth failing it over. Everything that CAN be
/// rejected was rejected before the first write.
///
/// Before this, the field was written in exactly two places — trip creation
/// (only when the create payload named a driver) and `update_trip_metadata`,
/// which is MCP/REST-only. Plan-then-assign therefore never got a link, and
/// since #433 made auto-dispatch chain-only, those trips silently stopped
/// auto-dispatching with nothing in the UI to explain why or to fix it.
async fn resolve_chain_link(
    state: &AppState,
    trip: TripRecord,
    req: &AssignTripRequest,
    derive_allowed: bool,
) -> TripRecord {
    // A settled trip's miles are frozen; an explicit value was already rejected
    // above, so anything reaching here would be a derivation and must not run.
    if trip.settlement_ref.is_some() {
        return trip;
    }

    let resolved = match req.previous_trip_id {
        // The dispatcher said what they wanted, `null` included.
        Some(explicit) => explicit,
        None => {
            // A link pointing at another driver's trip cannot fire and blocks
            // this trip permanently — that is a stale link from a previous
            // assignment, not a plan, so re-derive rather than preserve it.
            let stale = match trip.previous_trip_id {
                Some(prev) => !predecessor_usable_by(state, prev, req.driver_id).await,
                None => false,
            };
            if trip.previous_trip_id.is_some() && !stale {
                // A usable link is left alone whether a dispatcher stated it or
                // an earlier assign derived it — the two are indistinguishable on
                // the record, and repointing either would move the deadhead
                // origin, and with it driver pay.
                return trip;
            }
            if !derive_allowed && !stale {
                return trip;
            }
            // Something already queued behind this trip means it is a predecessor,
            // not a tail; deriving would point it at its own successor.
            match state.db.list_trips_referencing_previous(trip.id).await {
                Ok(succ) if succ.iter().any(|t| !is_terminal_for_chaining(&t.status)) => {
                    return trip;
                }
                Err(e) => {
                    tracing::warn!(trip_id = %trip.id, error = %e,
                        "assign: could not check for successors; leaving the chain link alone");
                    return trip;
                }
                Ok(_) => {}
            }
            match state.db.get_chain_tail_for_driver(req.driver_id, trip.id).await {
                Ok(prev) => prev.map(|t| t.id),
                Err(e) => {
                    tracing::warn!(trip_id = %trip.id, error = %e,
                        "assign: could not look up a chain predecessor; leaving the trip unchained");
                    return trip;
                }
            }
        }
    };

    if resolved == trip.previous_trip_id {
        return trip;
    }

    if let Err(e) = state.db.update_trip_previous_trip_id(trip.id, resolved).await {
        tracing::warn!(trip_id = %trip.id, error = %e, "assign: chain link not persisted");
        return trip;
    }

    // Recompute is best-effort for the same reason it is in `apply_trip_patch`:
    // the link is committed and valuable on its own, and ORS being down must not
    // turn an assignment into a failure. Unlike that path there is no response
    // field to carry a warning, so the failure is journalled — stale miles feed
    // driver pay, and a log line is not a record anyone reviews.
    if let Err(e) = crate::api::trips::compute_and_persist_mileage(state, trip.id).await {
        tracing::warn!(trip_id = %trip.id, error = %e,
            "assign: mileage recompute after chain link change failed");
        events::on_mileage_recompute_failed(&state.db, trip.id, &e.to_string()).await;
    }

    state.db.get_trip(trip.id).await.unwrap_or(trip)
}

/// Whether an existing link is still usable for `driver_id`. Only a link to a
/// DIFFERENT driver's trip is stale.
///
/// A predecessor with no driver is not stale: that is the ordinary chain origin
/// the create path writes, and it exists to supply the deadhead origin for
/// mileage rather than to dispatch anything. Clearing it would erase a routing
/// input nobody asked to change. An unreadable predecessor is left alone too —
/// `validate_chain_link` stops new dangling links, and silently rewriting old
/// data on an unrelated code path is not this function's job.
async fn predecessor_usable_by(state: &AppState, prev_id: Uuid, driver_id: Uuid) -> bool {
    match state.db.get_trip(prev_id).await {
        Ok(t) => t.driver_id.is_none() || t.driver_id == Some(driver_id),
        Err(_) => true,
    }
}

pub async fn unassign(state: &AppState, trip_id: Uuid) -> Result<TripRecord, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    state.db.transition_trip_status(trip_id, TripStatus::Planned).await?;
    state.db.update_trip_resources(trip_id, None, None, vec![]).await?;

    // Release each resource to Available — but never demote one that is currently
    // Dispatched. Since assign now accepts a still-dispatched resource onto a
    // planned follow-on, a resource on this (Assigned) trip can be live on ANOTHER
    // trip; unassigning here must not knock it back to Available and defeat the
    // single-active-dispatch guard at dispatch time. (unassign only runs on an
    // Assigned trip, so a Dispatched status here always means "active elsewhere".)
    if let Some(driver_id) = existing.driver_id {
        if let Ok(d) = state.db.get_driver_by_id(driver_id).await {
            if d.status != DriverStatus::Dispatched {
                let _ = state.db.update_driver_status(driver_id, DriverStatus::Available).await;
            }
        }
    }
    if let Some(truck_id) = existing.truck_id {
        if let Ok(t) = state.db.get_truck_by_id(truck_id).await {
            if t.status != TruckStatus::Dispatched {
                let _ = state.db.update_truck_status(truck_id, TruckStatus::Available).await;
            }
        }
    }
    for &trailer_id in &existing.trailer_ids {
        if let Ok(tr) = state.db.get_trailer_by_id(trailer_id).await {
            if tr.status != TrailerStatus::Dispatched {
                let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Available).await;
            }
        }
    }

    // The trip is back to Planned, so it no longer holds the load. Count only
    // holding trips (assigned/dispatched/in_transit) — counting planned ones would
    // include the trip we just released, leaving the load stuck at `assigned` with
    // nothing actually holding it.
    if let Some(load_id) = existing.load_id {
        let holding = state.db.count_load_holding_trips(load_id).await.unwrap_or(1);
        if holding == 0 {
            if let Ok(load) = state.db.get_load_by_id(load_id).await {
                if load.status == LoadStatus::Assigned {
                    demote_released_load(state, load_id, LoadStatus::Planned).await;
                }
            }
        }
    }

    let trip = state.db.get_trip(trip_id).await?;
    events::on_trip_unassigned(&state.db, trip_id).await;
    Ok(trip)
}

pub async fn dispatch(state: &AppState, trip_id: Uuid) -> Result<TripRecord, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    if existing.status != TripStatus::Assigned {
        return Err(AppError::Conflict("trip must be in assigned status to dispatch".into()));
    }

    // #433: warn, but proceed. A dispatcher starting a trip out of chain order
    // is overriding the plan on purpose — that is their call to make, unlike
    // the automatic path, which hard-blocks because nobody is watching it.
    if let Err(reason) = predecessor_blocking_dispatch(state, existing.previous_trip_id).await {
        tracing::warn!(%trip_id, %reason, "dispatching a trip whose predecessor has not finished");
    }

    let driver_for_dispatch = if let Some(driver_id) = existing.driver_id {
        let driver = state.db.get_driver_by_id(driver_id).await?;
        if driver.status == DriverStatus::Dispatched {
            return Err(AppError::Conflict(
                "driver is already dispatched on another trip".into()
            ));
        }
        Some(driver)
    } else {
        None
    };
    if let Some(truck_id) = existing.truck_id {
        let truck = state.db.get_truck_by_id(truck_id).await?;
        if truck.status == TruckStatus::Dispatched {
            return Err(AppError::Conflict(
                "truck is already dispatched on another trip".into()
            ));
        }
    }

    // Reconcile trip trailers to the driver's currently-attached trailers.
    // Issue #268: at dispatch time, the trip should reflect reality — the trailer
    // physically attached to the driver — not the trailer the trip was created with.
    let mut existing = existing;
    if let Some(driver) = &driver_for_dispatch {
        if !driver.current_trailer_ids.is_empty()
            && driver.current_trailer_ids != existing.trailer_ids
        {
            let dropped: Vec<Uuid> = existing.trailer_ids.iter()
                .filter(|tid| !driver.current_trailer_ids.contains(tid))
                .copied()
                .collect();
            state.db.update_trip_resources(
                existing.id,
                existing.driver_id,
                existing.truck_id,
                driver.current_trailer_ids.clone(),
            ).await?;
            existing.trailer_ids = driver.current_trailer_ids.clone();
            // Trailers that were assigned to this trip but are no longer attached
            // fall back to Available — they're no longer on this load.
            for tid in dropped {
                let _ = state.db.update_trailer_status(tid, TrailerStatus::Available).await;
            }
        }
    }

    let trip = state.db.transition_trip_status(trip_id, TripStatus::Dispatched).await?;

    if let Some(driver_id) = existing.driver_id {
        let _ = state.db.update_driver_status(driver_id, DriverStatus::Dispatched).await;
    }
    if let Some(truck_id) = existing.truck_id {
        let _ = state.db.update_truck_status(truck_id, TruckStatus::Dispatched).await;
    }
    for &trailer_id in &existing.trailer_ids {
        let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Dispatched).await;
    }

    if let Some(load_id) = existing.load_id {
        if let Ok(load) = state.db.get_load_by_id(load_id).await {
            if load.status == LoadStatus::Assigned {
                let _ = state.db.transition_load_status(load_id, LoadStatus::Dispatched, None, None, None, None).await;
            }
        }
    }

    // Manual path: no actor yet — `dispatch_trip` does not thread caller identity
    // through, and a guessed actor is worse than an absent one (#435).
    events::on_trip_dispatched(&state.db, trip_id, None, None).await;
    Ok(trip)
}

pub async fn undispatch(state: &AppState, trip_id: Uuid) -> Result<TripRecord, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    if existing.status != TripStatus::Dispatched {
        return Err(AppError::Conflict("trip must be in dispatched status to undispatch".into()));
    }

    let trip = state.db.transition_trip_status(trip_id, TripStatus::Assigned).await?;

    if let Some(driver_id) = existing.driver_id {
        let _ = state.db.update_driver_status(driver_id, DriverStatus::Assigned).await;
    }
    if let Some(truck_id) = existing.truck_id {
        let _ = state.db.update_truck_status(truck_id, TruckStatus::Assigned).await;
    }
    for &trailer_id in &existing.trailer_ids {
        let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Assigned).await;
    }

    if let Some(load_id) = existing.load_id {
        if let Ok(all_trips) = state.db.list_trips_for_load(load_id).await {
            let any_dispatched = all_trips.iter().any(|t| {
                t.id != trip_id && (t.status == TripStatus::Dispatched || t.status == TripStatus::InTransit)
            });
            if !any_dispatched {
                if let Ok(load) = state.db.get_load_by_id(load_id).await {
                    if load.status == LoadStatus::Dispatched {
                        demote_released_load(state, load_id, LoadStatus::Assigned).await;
                    }
                }
            }
        }
    }

    events::on_trip_undispatched(&state.db, trip_id).await;
    Ok(trip)
}

pub async fn cancel(state: &AppState, trip_id: Uuid) -> Result<TripRecord, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    if existing.status == TripStatus::InTransit || existing.status == TripStatus::Delivered {
        return Err(AppError::Conflict("cannot cancel a trip that is in_transit or delivered".into()));
    }

    let trip = state.db.transition_trip_status(trip_id, TripStatus::Cancelled).await?;

    if let Some(driver_id) = existing.driver_id {
        let _ = state.db.update_driver_status(driver_id, DriverStatus::Available).await;
    }
    if let Some(truck_id) = existing.truck_id {
        let _ = state.db.update_truck_status(truck_id, TruckStatus::Available).await;
    }
    for &trailer_id in &existing.trailer_ids {
        let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Available).await;
    }

    // Same holding-trip rule as `unassign`: a leftover *planned* sibling trip
    // doesn't hold the load, so it must not pin the load at `assigned`. Cancelling
    // reaches here from Assigned OR Dispatched — a dispatched load whose only trip
    // is cancelled has nothing holding it either, and covering only Assigned left
    // it stranded at `dispatched`. A load already Planned needs no move (and
    // Planned -> Planned is not a transition).
    if let Some(load_id) = existing.load_id {
        let holding = state.db.count_load_holding_trips(load_id).await.unwrap_or(1);
        if holding == 0 {
            if let Ok(load) = state.db.get_load_by_id(load_id).await {
                if matches!(load.status, LoadStatus::Assigned | LoadStatus::Dispatched) {
                    demote_released_load(state, load_id, LoadStatus::Planned).await;
                }
            }
        }
    }

    events::on_trip_cancelled(&state.db, trip_id).await;
    Ok(trip)
}

/// Resolve a `waypoint` position. The stop type is **forced** to `Waypoint`,
/// whatever the caller sent.
///
/// `PositionInput` is shared between the `waypoint` field and `divert`'s `stops`
/// array. The `stop_type` override is legitimate there (a cross-dock hand-off is
/// a `relay`, not a `delivery`) and has no valid use on a waypoint, where
/// honouring it defeats two guards: `cascade_final_stop_delivered` keys on
/// `stop_type == Waypoint`, so a `divert` with `waypoint.stop_type: "delivery"`
/// and empty `stops` produces a trip whose max-sequence stop is a `Delivery` —
/// departing it marks the trip `Delivered` and cascades the load to `Delivered`,
/// for freight that never arrived anywhere; and on `tonu` a service type earns
/// the driver an extra-stop fee for parking.
async fn resolve_waypoint_position(
    state: &AppState,
    mut pos: PositionInput,
) -> Result<crate::models::TripStop, AppError> {
    pos.stop_type = None;
    resolve_position(state, pos, "waypoint", crate::models::TripStopType::Waypoint).await
}

/// Resolve a `PositionInput` into a trip stop typed `default_stop_type` unless
/// the input overrides it. Runs before any mutation so a geocode failure cannot
/// leave a half-applied outcome behind.
///
/// Waypoints do not go through here directly — see [`resolve_waypoint_position`],
/// which strips the override first.
///
/// `field` names the request field this position came from (`waypoint`,
/// `stops[1]`, …) so a rejection points at the value the caller actually sent —
/// `divert` feeds several positions through here, and a message that always said
/// "waypoint" sent the caller to the wrong one.
///
/// `sequence` is left at 0: every caller renumbers the whole list by position
/// after appending, so anything set here would be overwritten unread.
pub(crate) async fn resolve_position(
    state: &AppState,
    pos: PositionInput,
    field: &str,
    default_stop_type: crate::models::TripStopType,
) -> Result<crate::models::TripStop, AppError> {
    use crate::models::TripStop;

    let _: chrono_tz::Tz = pos.timezone.parse().map_err(|_| {
        AppError::UnprocessableEntity(format!("'{}' is not a valid IANA timezone", pos.timezone))
    })?;
    for (name, value) in [("actual_arrive", &pos.actual_arrive), ("actual_depart", &pos.actual_depart)] {
        if let Some(v) = value {
            crate::models::load::validate_stop_time_str(v, &pos.timezone, name)?;
        }
    }

    let (facility_id, name, address) = match pos.facility_id {
        Some(id) => {
            let f = state.db.get_facility_by_id(id).await?;
            (id, f.name, f.address)
        }
        None => {
            let name = pos.facility_name.ok_or_else(|| AppError::UnprocessableEntity(
                format!("{field} must provide either facility_id or facility_name + address")
            ))?;
            let address = pos.address.ok_or_else(|| AppError::UnprocessableEntity(
                format!("{field} must provide address when facility_id is not given")
            ))?;
            let id = resolve_waypoint_facility(state, &name, &address).await?;
            (id, name, address)
        }
    };

    Ok(TripStop {
        sequence: 0,
        stop_type: pos.stop_type.unwrap_or(default_stop_type),
        facility_id: Some(facility_id),
        name: Some(name),
        address: Some(address),
        load_stop_index: None,
        scheduled_arrive: None,
        scheduled_arrive_end: None,
        actual_arrive: pos.actual_arrive,
        actual_depart: pos.actual_depart,
        expected_dwell_minutes: None,
        detention_free_minutes: None,
        detention_grace_minutes: None,
        notes: pos.notes,
        timezone: Some(pos.timezone),
        actual_arrive_utc: None,
        actual_depart_utc: None,
    })
}

/// Dedup a waypoint's name+address against existing facilities, falling back to
/// an outright create when dedup itself is unavailable.
///
/// `resolve_or_create_facility` needs an embedding to search, so it fails closed
/// when Ollama is down — correct for a booked load, wrong here: a truck released
/// at the dock is an operational fact that must be recordable regardless. An
/// ambiguous match still propagates, since only the caller can pick between the
/// candidates it names.
async fn resolve_waypoint_facility(
    state: &AppState, name: &str, address: &str,
) -> Result<Uuid, AppError> {
    match crate::api::facilities::resolve_or_create_facility(state, name, address, false).await {
        Ok(id) => Ok(id),
        Err(e @ AppError::FacilityResolution(_)) => Err(e),
        Err(e) => {
            tracing::warn!(%name, error = %e, "waypoint facility dedup unavailable; creating a new facility");
            crate::api::facilities::resolve_or_create_facility(state, name, address, true).await
        }
    }
}

/// Recompute mileage and reassign the whole figure to deadhead.
///
/// `compute_trip_mileage` calls a leg deadhead only when it originates from a
/// `previous_trip_id`; every other leg is loaded. On a TONU that empty run *is*
/// the entire trip, so delegating the split would pay 100% of the miles at the
/// loaded rate. Stale planned mileage is cleared first, so a routing failure
/// leaves an honest "unknown" rather than the never-driven loaded figure.
async fn recompute_as_all_deadhead(state: &AppState, trip_id: Uuid) -> Option<String> {
    if let Err(e) = state.db.update_trip_mileage(trip_id, None, None, None, vec![]).await {
        tracing::warn!(%trip_id, error = %e, "stale planned mileage not cleared before TONU recompute");
    }
    if let Err(e) = crate::api::trips::compute_and_persist_mileage(state, trip_id).await {
        return Some(format!("mileage not recomputed: {e}"));
    }
    reassign_all_to_deadhead(state, trip_id).await
}

/// Move a just-routed trip's whole figure into `deadhead_miles`, leaving
/// `loaded_miles` `None`.
///
/// Split out of [`recompute_as_all_deadhead`] because `recalculate_trip_miles`
/// needs the same reassignment: that handler calls `compute_and_persist_mileage`
/// directly, and its `already_set` short-circuit does not fire on a TONU trip
/// whose mileage is `None` because ORS was down when it was recorded — which is
/// precisely the recovery path TONU documents. Without this, the documented
/// repair puts the empty run in `loaded_miles` and `compute_driver_pay` bills it
/// at the loaded rate.
pub(crate) async fn reassign_all_to_deadhead(state: &AppState, trip_id: Uuid) -> Option<String> {
    let t = match state.db.get_trip(trip_id).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(%trip_id, error = %e, "trip not re-read; miles left split as routed");
            return Some(format!("miles routed but not reassigned to deadhead: {e}"));
        }
    };
    let total = t.total_miles
        .or_else(|| match (t.deadhead_miles, t.loaded_miles) {
            (None, None) => None,
            (d, l) => Some(d.unwrap_or(0.0) + l.unwrap_or(0.0)),
        });
    // Discarding this write would leave the whole empty run sitting in
    // `loaded_miles` — the exact state this helper exists to prevent, and one the
    // caller would otherwise pay out at the loaded rate without ever being told.
    if let Err(e) = state.db.update_trip_mileage(trip_id, total, None, total, t.segment_miles).await {
        tracing::warn!(%trip_id, error = %e, "deadhead reassignment failed; miles stay split as routed");
        return Some(format!("miles routed but not reassigned to deadhead: {e}"));
    }
    None
}

/// TONU — Truck Ordered Not Used. Valid only from `Dispatched`: the truck rolled
/// but never departed a pickup, so no freight was ever aboard.
pub async fn tonu(
    state: &AppState,
    trip_id: Uuid,
    req: TonuRequest,
) -> Result<TripOutcomeResult, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    match existing.status {
        TripStatus::Dispatched => {}
        TripStatus::Planned | TripStatus::Assigned => {
            return Err(AppError::Conflict(
                "trip has not been dispatched; no truck was used — use cancel_trip".into()));
        }
        TripStatus::InTransit => {
            return Err(AppError::Conflict(
                "trip has departed its pickup and is carrying freight — use divert_trip".into()));
        }
        s => {
            return Err(AppError::Conflict(format!(
                "cannot TONU a trip with status '{}'", s.as_str())));
        }
    }
    if existing.settlement_ref.is_some() {
        return Err(AppError::Conflict("trip is settled; miles and pay are frozen".into()));
    }

    // Everything that can be rejected is decided before the first write.
    let last_reached = existing.stops.iter()
        .filter(|s| s.actual_arrive.is_some())
        .max_by_key(|s| s.sequence)
        .map(|s| s.sequence);
    if last_reached.is_none() && req.waypoint.is_none() {
        return Err(AppError::UnprocessableEntity(
            "no stop was reached, so the truck's position is unknown: supply `waypoint` \
             with where the driver stopped, or the deadhead cannot be measured".into()));
    }

    let mut stops: Vec<crate::models::TripStop> = match last_reached {
        Some(seq) => existing.stops.iter().filter(|s| s.sequence <= seq).cloned().collect(),
        None => vec![],
    };
    // `last_reached` is derived by `sequence`, but everything below picks and
    // renumbers by vector position — and nothing in the codebase sorts trip
    // stops (`src/api/trips.rs` stores whatever order the caller supplied). Sort
    // so the two agree; without it a trip whose stops arrived out of order gets
    // the release time stamped on the wrong stop and the rest renumbered wrongly.
    stops.sort_by_key(|s| s.sequence);

    // `occurred_at` stamps the truncation stop — the last stop the truck reached
    // and has not left — and is parsed in that stop's own timezone. Where there
    // is no such stop there is neither somewhere to put it nor a zone to read it
    // in, and guessing the waypoint's zone would shift a detention clock by hours.
    // Reject instead of dropping it: the waypoint carries its own `actual_depart`
    // for exactly this case, and a 200 that silently ignored the field would read
    // as "applied".
    let release_stop_awaits = stops.last()
        .is_some_and(|s| s.actual_arrive.is_some() && s.actual_depart.is_none());
    if req.occurred_at.is_some() && !release_stop_awaits {
        return Err(AppError::UnprocessableEntity(
            "`occurred_at` stamps the last stop the truck reached and has not left, and \
             this trip has no such stop: put the release time in `waypoint.actual_depart`".into()));
    }

    // Stamp the release time on the truncation stop so the dock wait is billable.
    if let Some(last) = stops.last_mut() {
        if last.actual_arrive.is_some() && last.actual_depart.is_none() {
            let tz = last.timezone.as_deref().unwrap_or("UTC");
            let released = match &req.occurred_at {
                Some(v) => {
                    crate::models::load::validate_stop_time_str(v, tz, "occurred_at")?;
                    v.clone()
                }
                None => now_local_naive(tz),
            };
            last.actual_depart = Some(released);
        }
    }

    if let Some(pos) = req.waypoint {
        stops.push(resolve_waypoint_position(state, pos).await?);
    }
    for (i, s) in stops.iter_mut().enumerate() { s.sequence = i as u32; }

    // --- writes ---
    state.db.update_trip_metadata(trip_id,
        crate::db::trip_ops::TripMetadataUpdate { stops: Some(stops), ..Default::default() },
    ).await?;
    state.db.transition_trip_status(trip_id, TripStatus::Tonu).await?;
    let warning = recompute_as_all_deadhead(state, trip_id).await;

    if let Some(load_id) = existing.load_id {
        let holding = state.db.count_load_holding_trips(load_id).await.unwrap_or(1);
        if holding == 0 {
            if let Ok(load) = state.db.get_load_by_id(load_id).await {
                if matches!(load.status, LoadStatus::Assigned | LoadStatus::Dispatched) {
                    if let Err(e) = state.db.transition_load_status(
                        load_id, LoadStatus::Tonu, None, None, req.reason.clone(), None,
                    ).await {
                        tracing::warn!(%load_id, error = %e, "load not moved to tonu");
                    } else {
                        archive_quoted_rates(state, load_id).await;
                    }
                }
            }
        }
    }

    release_resources(state, &existing).await;
    events::on_trip_tonu(&state.db, trip_id, req.reason).await;

    let trip = state.db.get_trip(trip_id).await?;
    Ok(TripOutcomeResult { trip, mileage_recompute_warning: warning })
}

/// Re-target an in-transit trip. The trip keeps running; only the plan changes.
///
/// Unlike TONU, `rate_items` are left alone for every reason: the line haul is
/// at least partly earned once the freight is aboard.
pub async fn divert(
    state: &AppState,
    trip_id: Uuid,
    req: DivertRequest,
) -> Result<TripOutcomeResult, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    match existing.status {
        TripStatus::InTransit => {}
        TripStatus::Dispatched => {
            return Err(AppError::Conflict(
                "trip has not departed its pickup, so no freight is aboard — use tonu_trip \
                 to end it, or update_trip to change a stop it has not reached".into()));
        }
        s => {
            return Err(AppError::Conflict(format!(
                "cannot divert a trip with status '{}'", s.as_str())));
        }
    }
    if existing.settlement_ref.is_some() {
        return Err(AppError::Conflict("trip is settled; miles and pay are frozen".into()));
    }

    // History is the contiguous prefix up to the last stop the driver reached —
    // NOT every stop that happens to have an actual_arrive. Filtering on the
    // predicate alone would let a later arrived-at stop survive while dropping an
    // earlier unreached one, producing a stop list that never happened in that order.
    let last_reached = existing.stops.iter()
        .filter(|s| s.actual_arrive.is_some())
        .max_by_key(|s| s.sequence)
        .map(|s| s.sequence);
    let mut stops: Vec<crate::models::TripStop> = match last_reached {
        Some(seq) => existing.stops.iter().filter(|s| s.sequence <= seq).cloned().collect(),
        None => vec![],
    };
    // Same reason as `tonu`: the prefix is selected by `sequence` but renumbered
    // by vector position, and stored trip stops are never sorted.
    stops.sort_by_key(|s| s.sequence);

    // The invariant is that the kept list ends at the last position the truck
    // actually reached — that is what makes the appended stops a continuation of
    // the route rather than a shortcut across it. A trip already ending at a
    // reached `Waypoint` (where a hold-only divert left it) satisfies that
    // outright, so it needs no fresh divergence point, and its fully-arrived stop
    // list is not the dead end the guard below is written for.
    let ends_at_reached_waypoint = stops.last()
        .is_some_and(|s| s.stop_type == crate::models::TripStopType::Waypoint);
    if req.waypoint.is_none() && !ends_at_reached_waypoint {
        return Err(AppError::UnprocessableEntity(
            "`waypoint` is required: it marks where the old plan and the new plan \
             diverged, and routing walks waypoint to waypoint — without it the \
             recomputed route runs straight to the new destination and every mile \
             already driven toward the old one silently disappears. It may only be \
             omitted when the trip already ends at a waypoint the driver reached — \
             and it is `actual_arrive` on that waypoint that makes it count as \
             reached; a waypoint with no arrival time is not in the kept history."
                .into()));
    }
    if !ends_at_reached_waypoint && stops.len() == existing.stops.len() && !existing.stops.is_empty() {
        return Err(AppError::UnprocessableEntity(
            "this trip's last stop has been arrived at and is not a waypoint, so there \
             is nothing after it left to replace; clear the actuals on the stop you mean \
             to rewrite before diverting".into()));
    }

    // Resolve every position before the first write. `sequence` is not passed:
    // the renumber below is what assigns it.
    if let Some(pos) = req.waypoint {
        stops.push(resolve_waypoint_position(state, pos).await?);
    }
    for (i, pos) in req.stops.into_iter().enumerate() {
        // Destinations default to `delivery`; a cross-dock hand-off can override
        // to `relay` via the position's own `stop_type`.
        stops.push(resolve_position(
            state, pos, &format!("stops[{i}]"), crate::models::TripStopType::Delivery,
        ).await?);
    }
    for (i, s) in stops.iter_mut().enumerate() { s.sequence = i as u32; }

    // --- writes ---
    state.db.update_trip_metadata(trip_id,
        crate::db::trip_ops::TripMetadataUpdate { stops: Some(stops.clone()), ..Default::default() },
    ).await?;
    // Clear before recomputing, exactly as `tonu` does. The old figure measures a
    // route to a consignee this trip is no longer going to, and it is not merely
    // cosmetic: `recalculate_trip_miles` short-circuits when deadhead and loaded
    // are both already set (`trip_writes.rs`, unless the caller knows to pass
    // `force`), and miles are not hand-settable — so a stale figure surviving an
    // ORS outage here survives every later attempt to correct it, and
    // `compute_driver_pay` bills the per-mile driver against it.
    if let Err(e) = state.db.update_trip_mileage(trip_id, None, None, None, vec![]).await {
        tracing::warn!(%trip_id, error = %e, "superseded mileage not cleared before divert recompute");
    }
    let warning = match crate::api::trips::compute_and_persist_mileage(state, trip_id).await {
        Ok(_) => None,
        Err(e) => Some(format!("mileage not recomputed: {e}")),
    };

    if req.reason.flags_the_load() {
        if let Some(load_id) = existing.load_id {
            if let Err(e) = state.db.mark_load_diverted(
                load_id, req.reason.as_str(), req.notes.clone(),
            ).await {
                tracing::warn!(%load_id, error = %e, "load not flagged as diverted");
            }
        }
    }

    events::on_trip_diverted(
        &state.db, trip_id, req.reason.as_str(), req.notes, stops.len(),
    ).await;

    let trip = state.db.get_trip(trip_id).await?;
    Ok(TripOutcomeResult { trip, mileage_recompute_warning: warning })
}

/// Move `rate_items` into `quoted_rate_items` and clear them. The line haul will
/// never be earned, and a `tonu` load still reporting it is exactly the
/// false-positive class administrative loads were built to kill.
async fn archive_quoted_rates(state: &AppState, load_id: Uuid) {
    if let Err(e) = state.db.archive_load_rate_items(load_id).await {
        tracing::warn!(%load_id, error = %e, "quoted rate items not archived");
    }
}

/// Release driver, truck and trailers, skipping any already rebound to another
/// active trip. Shared with `complete`.
async fn release_resources(state: &AppState, existing: &TripRecord) {
    let active = list_active_trips(state).await.unwrap_or_default();
    if let Some(driver_id) = existing.driver_id {
        if !resource_on_other_active_trip(&active, existing.id, Some(driver_id), None, None) {
            let _ = state.db.update_driver_status(driver_id, DriverStatus::Available).await;
        }
    }
    if let Some(truck_id) = existing.truck_id {
        if !resource_on_other_active_trip(&active, existing.id, None, Some(truck_id), None) {
            let _ = state.db.update_truck_status(truck_id, TruckStatus::Available).await;
        }
    }
    for &trailer_id in &existing.trailer_ids {
        if !resource_on_other_active_trip(&active, existing.id, None, None, Some(trailer_id)) {
            let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Available).await;
        }
    }
}

/// Now, as a naive local datetime string in `tz` — the format every stop time
/// in this system uses.
fn now_local_naive(tz: &str) -> String {
    let zone: chrono_tz::Tz = tz.parse().unwrap_or(chrono_tz::UTC);
    chrono::Utc::now().with_timezone(&zone).format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Outcome of a `delete` call, so callers can tell the two-call soft-then-hard
/// semantics apart: the first call on an active trip `Cancelled` it (record and
/// its trip number still exist); a second call `Deleted` it for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
    Cancelled,
    Deleted,
}

/// Deletes a trip, always releasing its equipment. A non-terminal trip
/// (Planned/Assigned/Dispatched) is soft-cancelled via `cancel` — which
/// transitions it to Cancelled AND releases its driver/truck/trailers back to
/// Available — and an already-Cancelled trip is hard-deleted. This preserves
/// the two-call delete semantics of `db.delete_trip` while ensuring equipment
/// is never stranded in `assigned` after the owning trip is gone.
///
/// A hard-delete is refused while another trip still points at this one via
/// `previous_trip_id`, so deletion can't strand a dangling chain link; the
/// caller is told which trips to re-point or clear first.
pub async fn delete(state: &AppState, trip_id: Uuid) -> Result<DeleteOutcome, AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    match existing.status {
        TripStatus::Cancelled => {
            let referencing = state.db.list_trips_referencing_previous(trip_id).await?;
            if !referencing.is_empty() {
                let nums: Vec<&str> = referencing.iter().map(|t| t.trip_number.as_str()).collect();
                return Err(AppError::Conflict(format!(
                    "cannot delete trip: it is referenced as previous_trip_id by {}. \
                     Re-point or clear those trips first.",
                    nums.join(", ")
                )));
            }
            state.db.hard_delete_trip(trip_id).await?;
            return Ok(DeleteOutcome::Deleted);
        }
        // `Tonu` belongs here rather than in the soft-cancel path below: it is
        // terminal with no edge to `Cancelled`, so falling through would call
        // `cancel` and fail with a transition error naming neither verb.
        TripStatus::InTransit | TripStatus::Delivered | TripStatus::Completed | TripStatus::Tonu => {
            return Err(AppError::Conflict(format!(
                "cannot delete trip with status '{}'",
                existing.status.as_str()
            )));
        }
        _ => {}
    }
    // Planned / Assigned / Dispatched: soft-cancel, which releases equipment.
    cancel(state, trip_id).await?;
    Ok(DeleteOutcome::Cancelled)
}

/// Completes a delivered trip and releases its resources. Returns `()` because
/// the admin/dispatch surfaces respond 204 No Content.
pub async fn complete(state: &AppState, trip_id: Uuid) -> Result<(), AppError> {
    let existing = state.db.get_trip(trip_id).await?;
    if existing.status != TripStatus::Delivered {
        return Err(AppError::Conflict("trip must be in delivered status to complete".into()));
    }

    state.db.transition_trip_status(trip_id, TripStatus::Completed).await?;

    // Only release a resource to Available if it has NOT already been rebound
    // to another active trip (e.g. via auto-dispatch when this trip delivered).
    release_resources(state, &existing).await;

    events::on_trip_completed(&state.db, trip_id, existing.driver_id, existing.truck_id, &existing.trailer_ids).await;
    Ok(())
}

/// Records a stop-late flag by emitting the `stop.late` event. Verifies the trip
/// exists first.
pub async fn stop_late(
    state: &AppState,
    trip_id: Uuid,
    seq: u32,
    req: StopLateRequest,
) -> Result<(), AppError> {
    let trip = state.db.get_trip(trip_id).await?;
    events::on_stop_late(&state.db, &trip, seq, req.eta, req.notes).await;
    Ok(())
}

/// Records a check call by emitting the `check_call` event. Verifies the trip
/// exists first.
pub async fn check_call(
    state: &AppState,
    trip_id: Uuid,
    req: CheckCallRequest,
) -> Result<(), AppError> {
    state.db.get_trip(trip_id).await?;
    events::on_check_call(&state.db, trip_id, req.location, req.notes, req.eta_next_stop).await;
    Ok(())
}

/// Fetches all trips currently in Dispatched or InTransit status.
async fn list_active_trips(state: &AppState) -> Result<Vec<crate::models::trip::TripListItem>, AppError> {
    state.db.list_active_trips().await
}

/// Returns true if any trip in `active` (other than `exclude_trip_id`)
/// references `resource_id` via the resource-matching closure.
fn resource_on_other_active_trip(
    active: &[crate::models::trip::TripListItem],
    exclude_trip_id: Uuid,
    driver_id: Option<Uuid>,
    truck_id: Option<Uuid>,
    trailer_id: Option<Uuid>,
) -> bool {
    active.iter().any(|t| {
        if t.id == exclude_trip_id { return false; }
        if let Some(d) = driver_id { if t.driver_id == Some(d) { return true; } }
        if let Some(tk) = truck_id { if t.truck_id == Some(tk) { return true; } }
        if let Some(tr) = trailer_id { if t.trailer_ids.contains(&tr) { return true; } }
        false
    })
}

/// True when a trip status means the trip is over and a successor may start.
/// TONU counts: the truck rolled and was released, so the chain moves on.
pub(crate) fn is_terminal_for_chaining(status: &TripStatus) -> bool {
    // Matched exhaustively on purpose: a new TripStatus must be classified
    // here rather than defaulting to "still running" under a catch-all. The
    // terminal-status list has been missed twice on this codebase already.
    match status {
        TripStatus::Delivered
        | TripStatus::Completed
        | TripStatus::Cancelled
        | TripStatus::Tonu => true,
        TripStatus::Planned
        | TripStatus::Assigned
        | TripStatus::Dispatched
        | TripStatus::InTransit => false,
    }
}

/// Whether `trip`'s declared predecessor is finished. `Ok(())` when there is no
/// predecessor or it is terminal; `Err(reason)` when the predecessor is still
/// running, or cannot be read at all.
///
/// #433: a trip whose predecessor has not finished is, on its own record, not
/// yet eligible to start. Callers decide the consequence — the automatic path
/// hard-blocks, the manual path warns and proceeds, because a dispatcher
/// overriding the plan is a legitimate thing to do.
async fn predecessor_blocking_dispatch(
    state: &AppState,
    previous_trip_id: Option<Uuid>,
) -> Result<(), String> {
    let Some(prev_id) = previous_trip_id else { return Ok(()) };
    match state.db.get_trip(prev_id).await {
        Ok(prev) if is_terminal_for_chaining(&prev.status) => Ok(()),
        Ok(prev) => Err(format!(
            "previous trip {prev_id} is still {} (not delivered/completed/cancelled/tonu)",
            prev.status.as_str()
        )),
        Err(e) => Err(format!("previous trip {prev_id} could not be read: {e}")),
    }
}

/// After a trip transitions to Delivered, dispatch the successor the completed
/// trip actually names. Best-effort: errors are logged and swallowed so a
/// hiccup here does not break the calling endpoint.
///
/// **Selection is chain-only (#433).** A candidate is an Assigned trip on this
/// driver whose `previous_trip_id` is the trip that just delivered. Zero
/// candidates dispatches nothing, and more than one dispatches nothing and
/// journals the ambiguity — there is deliberately no recency or scheduled-time
/// fallback. The old ordering (earliest first-stop `scheduled_arrive`) put the
/// wrong load in a driver's app mid-run: on a lane that pairs a loaded run with
/// a follow-on empty move, both sit Assigned at once and broker appointment
/// times routinely arrive out of order. A wrong auto-dispatch is worse than
/// none, because it silently replaces what the driver sees.
///
/// **The zero-candidate case journals selectively (#438).** It emits an event
/// only when the driver has OTHER Assigned trips that did not qualify — work is
/// queued and the chain does not reach it, so nothing rolls until a dispatcher
/// intervenes. A driver with nothing queued is the ordinary end of a chain,
/// fires on most completions, and stays a log line.
///
/// `dispatch`'s resource-conflict checks are not reused as-is because the
/// driver and truck from the just-delivered trip will still read `Dispatched`.
/// Instead this helper checks whether the candidate trip's truck/trailers are
/// bound to ANOTHER active trip (not the one that just delivered) — if so, it
/// declines to auto-dispatch and leaves the trip Assigned for the fleet_user.
pub(crate) async fn try_auto_dispatch_next_for_driver(
    state: &AppState,
    driver_id: Uuid,
    just_delivered_trip_id: Uuid,
) {
    let Ok(trips) = state.db.list_trips(None, Some(driver_id), Some("assigned"), None, None).await else {
        tracing::warn!(%driver_id, "auto-dispatch: failed to list assigned trips");
        return;
    };
    // The just-delivered trip cannot be in this list — it is Delivered, not
    // Assigned — so the chain link is the only filter needed. The trips that do
    // NOT chain are kept rather than discarded: whether the driver has any is
    // what separates a normal end-of-chain from a stall (#438).
    let (candidates, unchained): (Vec<_>, Vec<_>) = trips.into_iter()
        .partition(|t| t.previous_trip_id == Some(just_delivered_trip_id));

    if candidates.is_empty() {
        // #438: "this driver is done for now" is the ordinary end of a chain and
        // fires on most completions — it stays a log line, because an event on
        // the common correct path trains the reader to ignore the feed. "This
        // driver has work queued but none of it chains off the trip that just
        // finished" is the actionable one: nothing rolls until a dispatcher
        // intervenes, and until now the only record was a log line nobody reads.
        if unchained.is_empty() {
            tracing::info!(
                %driver_id, prev_trip = %just_delivered_trip_id,
                "auto-dispatch: no assigned trip chains off this one and the driver has none queued"
            );
            return;
        }
        let ids: Vec<String> = unchained.iter().map(|t| t.id.to_string()).collect();
        tracing::warn!(
            %driver_id, prev_trip = %just_delivered_trip_id, queued = ?ids,
            "auto-dispatch: driver has assigned trips but none chain off this one; leaving dispatch to the fleet_user"
        );
        events::on_auto_dispatch_no_chain(&state.db, just_delivered_trip_id, driver_id, &ids).await;
        return;
    }
    if candidates.len() > 1 {
        let ids: Vec<String> = candidates.iter().map(|t| t.id.to_string()).collect();
        tracing::warn!(
            %driver_id, prev_trip = %just_delivered_trip_id, candidates = ?ids,
            "auto-dispatch: more than one assigned trip chains off this one; dispatching none"
        );
        events::on_auto_dispatch_ambiguous(&state.db, just_delivered_trip_id, &ids).await;
        return;
    }

    let next = &candidates[0];
    let trip_id = next.id;

    // Defence in depth behind the chain filter: refuse a successor whose own
    // predecessor has not finished. Redundant while selection is chain-only —
    // and that is the point, it survives a future fallback.
    if let Err(reason) = predecessor_blocking_dispatch(state, next.previous_trip_id).await {
        tracing::warn!(%trip_id, %reason, "auto-dispatch: predecessor not finished, skipping");
        return;
    }

    // Refuse to bind a truck or trailer that is already active on another trip.
    // The driver is exempt — they were on the just-delivered trip; their status
    // still reads Dispatched but that does not count as a different trip.
    let active = match list_active_trips(state).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(%trip_id, error = %e, "auto-dispatch: failed to list active trips");
            return;
        }
    };
    if let Some(truck_id) = next.truck_id {
        if resource_on_other_active_trip(&active, just_delivered_trip_id, None, Some(truck_id), None) {
            tracing::warn!(%trip_id, %truck_id, "auto-dispatch: truck busy on another active trip, skipping");
            return;
        }
    }
    for &trailer_id in &next.trailer_ids {
        if resource_on_other_active_trip(&active, just_delivered_trip_id, None, None, Some(trailer_id)) {
            tracing::warn!(%trip_id, %trailer_id, "auto-dispatch: trailer busy on another active trip, skipping");
            return;
        }
    }

    if let Err(e) = state.db.transition_trip_status(trip_id, TripStatus::Dispatched).await {
        tracing::warn!(%trip_id, error = %e, "auto-dispatch: trip state transition failed");
        return;
    }

    let _ = state.db.update_driver_status(driver_id, DriverStatus::Dispatched).await;
    if let Some(truck_id) = next.truck_id {
        let _ = state.db.update_truck_status(truck_id, TruckStatus::Dispatched).await;
    }
    for &trailer_id in &next.trailer_ids {
        let _ = state.db.update_trailer_status(trailer_id, TrailerStatus::Dispatched).await;
    }

    if let Some(load_id) = next.load_id {
        if let Ok(load) = state.db.get_load_by_id(load_id).await {
            if load.status == LoadStatus::Assigned {
                let _ = state.db.transition_load_status(
                    load_id, LoadStatus::Dispatched, None, None, None, Some(events::AUTO_DISPATCH_ACTOR),
                ).await;
            }
        }
    }

    tracing::info!(prev_trip = %just_delivered_trip_id, next_trip = %trip_id, %driver_id, "auto-dispatched next trip");
    events::on_trip_dispatched(
        &state.db, trip_id, Some(events::AUTO_DISPATCH_ACTOR), Some(just_delivered_trip_id),
    ).await;
}

#[cfg(test)]
mod tests {
    use super::is_terminal_for_chaining;
    use crate::models::TripStatus;

    /// Pins the terminal set the chain guard uses (#433). TONU belongs here:
    /// the truck rolled and was released, so the chain legitimately moves on.
    #[test]
    fn terminal_statuses_for_chaining() {
        for s in [
            TripStatus::Delivered,
            TripStatus::Completed,
            TripStatus::Cancelled,
            TripStatus::Tonu,
        ] {
            assert!(is_terminal_for_chaining(&s), "{s:?} should be terminal");
        }
        for s in [
            TripStatus::Planned,
            TripStatus::Assigned,
            TripStatus::Dispatched,
            TripStatus::InTransit,
        ] {
            assert!(!is_terminal_for_chaining(&s), "{s:?} should not be terminal");
        }
    }
}
