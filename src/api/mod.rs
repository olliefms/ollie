// src/api/mod.rs
pub mod auth;
pub mod blob;
pub mod utils;
pub mod version;
pub mod blobs;
pub mod dispatchers;
pub mod dispatcher_portal;
pub mod driver_portal;
pub mod drivers;
pub mod events;
pub mod facilities;
pub mod loads;
pub mod mileage_summary;
pub mod trailers;
pub mod trip_actions;
pub mod trips;
pub mod trucks;

use crate::{api::auth::require_bearer, models, AppState};
use axum::{
    extract::DefaultBodyLimit,
    middleware::from_fn,
    response::IntoResponse,
    routing::{delete, get, patch, post, put},
    Router,
};
use utoipa::OpenApi;
use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};

#[derive(OpenApi)]
#[openapi(
    paths(
        blobs::upload_blob,
        blobs::list_blobs,
        blob::get_blob,
        blob::update_blob,
        blob::delete_blob,
        blob::query_blob,
        facilities::create_facility,
        facilities::list_facilities,
        facilities::get_facility,
        facilities::update_facility,
        facilities::delete_facility,
        loads::create_load,
        loads::list_loads,
        loads::get_load,
        loads::update_load,
        loads::delete_load,
        loads::invoice_load,
        loads::cancel_load,
        loads::settle_load,
        trips::create_trip,
        trips::list_trips,
        trips::get_trip,
        trips::update_trip,
        trips::delete_trip,
        trip_actions::assign_trip,
        trip_actions::unassign_trip,
        trip_actions::dispatch_trip,
        trip_actions::undispatch_trip,
        trip_actions::cancel_trip,
        trip_actions::stop_arrive,
        trip_actions::stop_depart,
        trip_actions::stop_late,
        trip_actions::check_call,
        trip_actions::complete_trip,
        loads::load_stop_arrive,
        loads::load_stop_depart,
        events::list_events,
        events::get_event_handler,
        drivers::create_driver,
        drivers::list_drivers,
        drivers::get_driver,
        drivers::update_driver,
        drivers::delete_driver,
        drivers::set_driver_pin,
        trucks::create_truck,
        trucks::list_trucks,
        trucks::get_truck,
        trucks::update_truck,
        trucks::delete_truck,
        trailers::create_trailer,
        trailers::list_trailers,
        trailers::get_trailer,
        trailers::update_trailer,
        trailers::delete_trailer,
        dispatchers::create_dispatcher,
        dispatchers::list_dispatchers,
        dispatchers::get_dispatcher,
        dispatchers::update_dispatcher,
        dispatchers::reset_dispatcher_password,
        dispatcher_portal::auth::login,
        dispatcher_portal::auth::refresh,
        dispatcher_portal::data::list_loads,
        dispatcher_portal::data::get_load,
        dispatcher_portal::data::create_load,
        dispatcher_portal::data::update_load,
        dispatcher_portal::data::list_trips,
        dispatcher_portal::data::get_trip,
        dispatcher_portal::data::assign_trip,
        dispatcher_portal::data::unassign_trip,
        dispatcher_portal::data::dispatch_trip,
        dispatcher_portal::data::undispatch_trip,
        dispatcher_portal::data::cancel_trip,
        dispatcher_portal::data::complete_trip,
        dispatcher_portal::data::stop_arrive,
        dispatcher_portal::data::stop_depart,
        dispatcher_portal::data::stop_late,
        dispatcher_portal::data::check_call,
        dispatcher_portal::trip_writes::recalculate_miles_handler,
        dispatcher_portal::trip_writes::patch_trip_handler,
        dispatcher_portal::data::list_facilities,
        dispatcher_portal::data::get_facility,
        dispatcher_portal::facility_writes::create_facility_handler,
        dispatcher_portal::facility_writes::update_facility_handler,
        dispatcher_portal::data::list_drivers,
        dispatcher_portal::data::get_driver,
        dispatcher_portal::data::list_trucks,
        dispatcher_portal::data::get_truck,
        dispatcher_portal::data::list_trailers,
        dispatcher_portal::data::get_trailer,
        dispatcher_portal::data::list_events,
        dispatcher_portal::blobs::list_blobs,
        dispatcher_portal::blobs::upload_blob,
        dispatcher_portal::blobs::get_blob,
        dispatcher_portal::blobs::update_blob,
        dispatcher_portal::blobs::delete_blob,
        dispatcher_portal::blobs::query_blob,
        driver_portal::data::update_stop_times,
        driver_portal::equipment::get_equipment,
        driver_portal::equipment::update_trailer,
        driver_portal::equipment::list_available_trailers,
        driver_portal::documents::upload_document,
        driver_portal::documents::list_documents,
        driver_portal::documents::get_document_content,
        driver_portal::documents::delete_document,
        version::get_version,
    ),
    components(
        schemas(
            models::BlobStatus,
            models::BlobRecord,
            models::UpdateBlobRequest,
            models::BlobListItem,
            models::BlobListResponse,
            models::GeocodeStatus,
            models::FacilityContact,
            models::FacilityRecord,
            models::CreateFacilityRequest,
            models::UpdateFacilityRequest,
            models::FacilityListItem,
            models::FacilityListResponse,
            models::FacilityCandidate,
            models::FacilityResolutionResponse,
            models::StopType,
            models::ServiceType,
            models::LoadStatus,
            models::RateLineItem,
            models::Stop,
            models::StopInput,
            models::StopResponse,
            models::LoadRecord,
            models::CreateLoadRequest,
            models::UpdateLoadRequest,
            models::InvoiceActionRequest,
            models::CancelActionRequest,
            models::LoadListItem,
            models::LoadListResponse,
            models::LoadDetailResponse,
            blobs::BlobUploadRequest,
            blob::BlobQueryRequest,
            blob::BlobQueryResponse,
            models::EventResponse,
            models::EventListResponse,
            models::DriverStatus,
            models::DriverRecord,
            models::CreateDriverRequest,
            models::UpdateDriverRequest,
            models::SetDriverPinRequest,
            models::DriverListItem,
            models::DriverListResponse,
            models::TruckStatus,
            models::TruckRecord,
            models::CreateTruckRequest,
            models::UpdateTruckRequest,
            models::TruckListItem,
            models::TruckListResponse,
            models::TrailerOwner,
            models::TrailerStatus,
            models::TrailerRecord,
            models::CreateTrailerRequest,
            models::UpdateTrailerRequest,
            models::TrailerListItem,
            models::TrailerListResponse,
            models::TripStatus,
            models::TripStopType,
            models::TripStop,
            models::TripRecord,
            models::CreateTripRequest,
            models::UpdateTripRequest,
            models::TripListItem,
            models::TripListResponse,
            trip_actions::AssignTripRequest,
            trip_actions::StopArriveRequest,
            trip_actions::StopDepartRequest,
            trip_actions::StopLateRequest,
            trip_actions::CheckCallRequest,
            dispatcher_portal::trip_writes::RecalculateMilesBody,
            dispatcher_portal::trip_writes::PatchTripBody,
            dispatcher_portal::facility_writes::CreateFacilityBody,
            dispatcher_portal::facility_writes::PatchFacilityBody,
            loads::LoadStopArriveRequest,
            loads::LoadStopDepartRequest,
            driver_portal::data::DriverFacilityContact,
            driver_portal::data::UpdateStopTimesRequest,
            driver_portal::equipment::EquipmentTruckSummary,
            driver_portal::equipment::EquipmentTrailerSummary,
            driver_portal::equipment::DriverEquipmentResponse,
            driver_portal::equipment::UpdateTrailerRequest,
            driver_portal::equipment::UpdateTrailerResponse,
            driver_portal::equipment::AvailableTrailerItem,
            driver_portal::equipment::AvailableTrailersResponse,
            models::DispatcherStatus,
            models::DispatcherRecord,
            dispatchers::CreateDispatcherRequest,
            dispatchers::UpdateDispatcherRequest,
            dispatchers::ResetDispatcherPasswordRequest,
            dispatchers::DispatcherListResponse,
            dispatcher_portal::auth::LoginRequest,
            dispatcher_portal::auth::LoginResponse,
            dispatcher_portal::auth::LockResponse,
            version::VersionResponse,
        )
    ),
    modifiers(&SecurityAddon),
    info(
        title = "ollie API",
        version = "1.0.0",
        description = "RAG-enabled blob store and freight load management API. \
            All endpoints require Bearer auth except /openapi.json, /llms.txt, and /version."
    ),
    tags(
        (name = "meta", description = "Server metadata endpoints (unauthenticated)"),
        (name = "blobs", description = "Document blob storage with AI summarisation and semantic search"),
        (name = "dispatch", description = "Dispatcher portal data API — loads, trips, drivers, trucks, trailers, events"),
        (name = "dispatch-auth", description = "Dispatcher portal authentication — login and JWT refresh"),
        (name = "dispatchers", description = "Dispatcher admin CRUD and password management"),
        (name = "drivers", description = "Driver management with state machine"),
        (name = "events", description = "Append-only event journal (read-only)"),
        (name = "facilities", description = "Freight facility management with geocoding and semantic search"),
        (name = "loads", description = "Freight load lifecycle management"),
        (name = "trailers", description = "Trailer management with state machine"),
        (name = "trips", description = "Trip management with stop tracking and load cascade"),
        (name = "trucks", description = "Truck management with state machine"),
    )
)]
struct ApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "BearerAuth",
                SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
            );
        }
    }
}

async fn openapi_json() -> axum::Json<utoipa::openapi::OpenApi> {
    axum::Json(ApiDoc::openapi())
}

const LLMS_TXT: &str = r#"# ollie API

ollie is a REST API for freight load management with RAG-enabled document blob storage.

## Authentication

Public/unauthenticated endpoints:
  GET /version — Returns {"version":"x.y.z"} matching the server build (no auth).

All other endpoints except /openapi.json and /llms.txt require:
  Authorization: Bearer <ADMIN_API_KEY>

Missing or incorrect key returns 401 Unauthorized.

## Endpoint Groups

### Blobs — /api/v1/blobs, /api/v1/blob/:id
Store and retrieve files (PDFs, images, documents). Files are content-addressed and
deduplicated. Uploaded files are processed asynchronously: Ollama generates a text
summary and a vector embedding. Supports semantic search via ?s=<query>.

  POST   /api/v1/blobs              Upload file (multipart/form-data: file, name?, tags?).
                                    Returns 202 (accepted, queued for AI processing) for new files,
                                    or 201 (created, AI output copied) when an identical file was
                                    previously uploaded (content-addressed deduplication).
  GET    /api/v1/blobs              List or search blobs (?s=query for semantic search)
  GET    /api/v1/blob/:id           Download file or get JSON record (Accept: application/json)
  PUT    /api/v1/blob/:id           Update name and/or tags
  DELETE /api/v1/blob/:id           Delete (blocked if referenced by a load)
  POST   /api/v1/blobs/:id/query    Ask a natural-language question about the document.
                                    Body: { "prompt": "...", "model": "llama3.2" (optional) }
                                    Returns: { id, prompt, answer, model, processing_time_ms }
                                    Requires blob status=ready. Uses extracted text (text blobs)
                                    or vision model (scanned PDFs). 400 for bad prompt,
                                    422 if not ready or content type not queryable.

### Dispatcher Blobs — /dispatch/api/v1/blobs, /dispatch/api/v1/blob/:id
Same capabilities as the admin blob API, protected by dispatcher JWT.

  POST   /dispatch/api/v1/blobs              Upload file (multipart/form-data: file, name?, tags?)
  GET    /dispatch/api/v1/blobs              List or search blobs (?name=, ?s=, ?limit=, ?offset=)
  GET    /dispatch/api/v1/blob/:id           Download file or JSON record (Accept: application/json)
  PUT    /dispatch/api/v1/blob/:id           Update name and/or tags
  DELETE /dispatch/api/v1/blob/:id           Delete (409 if referenced by a load)
  POST   /dispatch/api/v1/blobs/:id/query    Natural-language question about a document

### Facilities — /api/v1/facilities, /api/v1/facilities/:id
Freight facilities (warehouses, loading docks). Address geocoding runs asynchronously.
Used as stop locations on loads. Supports semantic search.

  POST   /api/v1/facilities     Create facility
  GET    /api/v1/facilities     List or search facilities (?s=query for semantic search)
  GET    /api/v1/facilities/:id Get facility
  PATCH  /api/v1/facilities/:id Update facility fields
  DELETE /api/v1/facilities/:id Delete (blocked if referenced by a load)

### Loads — /api/v1/loads, /api/v1/loads/:id
Freight loads with multi-stop routes. Status lifecycle:
  planned → assigned → dispatched → in_transit → delivered → invoiced → settled
  (cancel is allowed from planned, assigned, dispatched, or in_transit)

Stop required fields: scheduled_arrive (naive local datetime, e.g. "2026-05-10T08:00:00"),
timezone (IANA tz string, e.g. "America/Chicago"). Both must be present together — a stop
with time but no timezone, or timezone but no time, is rejected (422).
Stop optional fields: scheduled_arrive_end (window close; null = strict appointment),
actual_arrive, actual_depart, expected_dwell_minutes, detention_free_minutes (default 120),
detention_grace_minutes (default 15). Detention eligibility: FCFS stops (scheduled_arrive_end
set) are eligible if actual_depart > actual_arrive + detention_free_minutes. Strict stops
are eligible only if actual_arrive ≤ scheduled_arrive + grace_minutes (early = on-time).
Time strings are stored as naive local datetimes; timezone is the authoritative offset source.
Legacy stops (pre-v1.3.3) have timezone: null and times stored as UTC — not silently converted.

  POST   /api/v1/loads          Create load
  GET    /api/v1/loads          List or search loads (?s, ?status, ?customer, ?from, ?to, ?tag)
  GET    /api/v1/loads/:id      Get load detail
  PATCH  /api/v1/loads/:id      Update load fields (unknown fields in the request body are silently ignored)
  DELETE /api/v1/loads/:id      Delete load (409 if load has active trips — cancel or complete them first)

  POST   /api/v1/loads/:id/invoice     Transition to invoiced (body: invoice_number?, invoice_date?)
  POST   /api/v1/loads/:id/cancel      Transition to cancelled (body: reason?)
  POST   /api/v1/loads/:id/settle      Transition to settled
  POST   /api/v1/loads/:id/stops/:seq/arrive   Record actual arrival at stop (body: actual_arrive)
  POST   /api/v1/loads/:id/stops/:seq/depart   Record actual departure from stop (body: actual_depart)

### Trips — /api/v1/trips, /api/v1/trips/:id
Trips represent the physical movement of a truck+driver on behalf of a load. A load may have
multiple trips (relay). Status lifecycle:
  planned → assigned (via /assign) → dispatched (via /dispatch) → in_transit → delivered
  (assign/dispatch are reversible; cancel allowed from planned, assigned, dispatched only;
   in_transit and delivered are terminal for cancel — use relay instead)

When a trip with both load_id and driver_id is created and the linked load is planned,
the load is automatically transitioned to assigned.

Trip responses now include operational data:
  previous_trip_id — auto-chained to driver's last non-cancelled trip, or dispatcher-provided
  deadhead_miles, loaded_miles — calculated via ORS HGV routing; null if unavailable or
    linked facilities lack coordinates
  load_number — denormalized from linked load at creation time

Trip stops now include:
  address — populated from linked facility at creation time

  POST   /api/v1/trips          Create trip (trip_number auto-generated as T-YYYY-NNNN if omitted).
                                BREAKING (v1.3.3): When `stops` is omitted or empty and `load_id`
                                is provided, stops are automatically inherited from the linked load.
                                To create a stopless trip linked to a load, provide `stops` with at
                                least one explicit entry.
  GET    /api/v1/trips          List trips (?load_id, ?driver_id, ?status, ?limit, ?offset)
  GET    /api/v1/trips/:id      Get trip record
  PATCH  /api/v1/trips/:id      Update trip fields (load_id, sequence, stops, notes)
  DELETE /api/v1/trips/:id      Two-step delete: first DELETE soft-cancels (transitions to cancelled; blocked if
                                in_transit, delivered, or completed → 204); second DELETE on an already-cancelled
                                trip hard-deletes the row (→ 204). GET after hard-delete returns 404.

  POST   /api/v1/trips/:id/assign           Assign driver, truck, trailers (body: driver_id, truck_id, trailer_ids?)
  POST   /api/v1/trips/:id/unassign         Unassign resources and revert to planned
  POST   /api/v1/trips/:id/dispatch         Dispatch trip (must be assigned)
  POST   /api/v1/trips/:id/undispatch       Revert dispatched trip to assigned
  POST   /api/v1/trips/:id/cancel           Cancel trip (blocked if in_transit or delivered)
  POST   /api/v1/trips/:id/complete         Complete trip (must be delivered; releases driver/truck/trailers back to available); returns 204
  POST   /api/v1/trips/:id/stops/:seq/arrive  Record actual arrival at stop (body: actual_arrive)
  POST   /api/v1/trips/:id/stops/:seq/depart  Record actual departure from stop (body: actual_depart); triggers trip/load status cascades
  POST   /api/v1/trips/:id/stops/:seq/late    Flag stop as late (body: eta?, notes?); returns 204
  POST   /api/v1/trips/:id/check-call         Record driver check-in (body: location, notes?, eta_next_stop?); returns 204

### Drivers — /api/v1/drivers, /api/v1/drivers/:id
Driver records with state machine. Status: available → assigned → dispatched (last two driven by trip events).
DELETE soft-deletes (sets status=inactive). PUT cannot set assigned or dispatched.

  POST   /api/v1/drivers              Create driver
  GET    /api/v1/drivers              List or search drivers (?s, ?status, ?limit, ?offset)
  GET    /api/v1/drivers/:id          Get driver
  PUT    /api/v1/drivers/:id          Update driver fields (cannot manually set assigned/dispatched)
  DELETE /api/v1/drivers/:id          Soft-delete (sets status=inactive)
  POST   /api/v1/drivers/:id/pin      Set driver PIN (body: pin — 4–6 numeric digits); returns 204.
                                      Used by dispatchers to provision portal access. Invalidates
                                      any outstanding driver JWTs.

### Trucks — /api/v1/trucks, /api/v1/trucks/:id
Truck records with state machine. Status: available → assigned → dispatched (assigned/dispatched driven by trip events).
out_of_service can be set/cleared via PUT. DELETE soft-deletes (sets status=inactive).

  POST   /api/v1/trucks          Create truck
  GET    /api/v1/trucks          List or search trucks (?s, ?status, ?limit, ?offset)
  GET    /api/v1/trucks/:id      Get truck
  PUT    /api/v1/trucks/:id      Update truck fields (out_of_service allowed; assigned/dispatched rejected)
  DELETE /api/v1/trucks/:id      Soft-delete (sets status=inactive)

### Trailers — /api/v1/trailers, /api/v1/trailers/:id
Trailer records with owner type (fleet|carrier|customer|other) and state machine.
Non-fleet trailers require owner_name. out_of_service can be set/cleared via PUT.
DELETE soft-deletes (sets status=inactive).

  POST   /api/v1/trailers          Create trailer (owner_name required if owner != fleet)
  GET    /api/v1/trailers          List or search trailers (?s, ?status, ?owner, ?limit, ?offset)
  GET    /api/v1/trailers/:id      Get trailer
  PUT    /api/v1/trailers/:id      Update trailer fields (out_of_service allowed; assigned/dispatched rejected)
  DELETE /api/v1/trailers/:id      Soft-delete (sets status=inactive)

### Dispatchers — /api/v1/dispatchers, /api/v1/dispatchers/:id
Dispatcher accounts for admin users. Email is normalized (lowercase + trimmed) and must be unique.
Passwords are hashed with bcrypt (cost 12). Token version increments on password reset to invalidate JWTs.

  POST   /api/v1/dispatchers              Create dispatcher (body: email, name, password). Returns 409 if email already in use.
  GET    /api/v1/dispatchers              List all dispatchers. Returns { dispatchers, returned }.
  GET    /api/v1/dispatchers/:id          Get dispatcher by UUID.
  PUT    /api/v1/dispatchers/:id          Update name and/or status (body: name?, status?).
  PUT    /api/v1/dispatchers/:id/password Admin reset password (body: password). Returns 204.

### Events — /api/v1/events, /api/v1/events/:id
Append-only event journal recording entity lifecycle transitions. Written by internal
pipeline workers; read-only via API. Timestamps are RFC3339 UTC+Z.

  GET    /api/v1/events          List events (?entity_id, ?entity_type, ?event_type, ?from, ?to)
  GET    /api/v1/events/:id      Get single event

## Facility Resolution

When creating or updating a load, stops can specify a facility by UUID (facility_id)
or by name + address. If any name+address matches are ambiguous, the API returns 200
with an array of FacilityResolutionResponse objects — one per ambiguous stop, each
with a stop_index field identifying which stop needs resolution. Retry the request
with facility_id set for each ambiguous stop, or set force_new_facility=true to
create a new facility for that stop.

## List vs. Search Response Counts

GET endpoints that support ?s= return a `returned` field.
- List mode (no ?s=): `returned` equals the total count of matching records (for pagination).
- Search mode (?s=query): `returned` equals the number of items in this response (bounded by limit).

## Dispatcher Portal

The dispatcher portal has its own auth namespace at /dispatch/auth/ and a data API
at /dispatch/api/v1/. These endpoints use JWT auth (not Bearer) and are intended for
the dispatcher web app — not for admin automation. Auth is email+password based with
bcrypt verification and exponential backoff lockout after 5 failed attempts
(15 min × 2^(failures-5), capped at 24h).

Auth endpoints (no auth required):
  POST /dispatch/auth/login    Authenticate with email+password; returns JWT on success.
                               Returns 423 with { error, locked_until } if account is locked.
  POST /dispatch/auth/refresh  Refresh an expiring JWT (must be within 7-day refresh window).

Data endpoints (dispatcher JWT required — same response shapes as admin API):
  GET  /dispatch/api/v1/loads              List loads (?status, ?customer, ?from, ?to, ?tag, ?limit, ?offset)
  GET  /dispatch/api/v1/loads/:id          Get load detail
  POST /dispatch/api/v1/loads              Create load (same fields as admin POST /api/v1/loads)
  PUT  /dispatch/api/v1/loads/:id          Update load fields

  GET  /dispatch/api/v1/trips              List trips (?load_id, ?driver_id, ?status)
  GET  /dispatch/api/v1/trips/:id          Get trip record
  POST /dispatch/api/v1/trips/:id/assign     Assign driver + truck + trailers (body: driver_id, truck_id, trailer_ids?)
  POST /dispatch/api/v1/trips/:id/unassign   Unassign resources and revert trip to planned
  POST /dispatch/api/v1/trips/:id/dispatch   Dispatch trip (must be assigned)
  POST /dispatch/api/v1/trips/:id/undispatch Revert dispatched trip to assigned
  POST /dispatch/api/v1/trips/:id/cancel     Cancel trip (blocked if in_transit or delivered)
  POST /dispatch/api/v1/trips/:id/complete   Complete trip (must be delivered; releases driver/truck/trailers); returns 204
  POST /dispatch/api/v1/trips/:id/stops/:seq/arrive  Record actual arrival at stop (body: actual_arrive)
  POST /dispatch/api/v1/trips/:id/stops/:seq/depart  Record actual departure (body: actual_depart); triggers trip/load status cascades
  POST /dispatch/api/v1/trips/:id/stops/:seq/late    Flag stop as late (body: eta?, notes?); returns 204
  POST /dispatch/api/v1/trips/:id/check-call         Record driver check-in (body: location, notes?, eta_next_stop?); returns 204

  GET  /dispatch/api/v1/drivers            List drivers (?status)
  GET  /dispatch/api/v1/drivers/:id        Get driver record

  GET  /dispatch/api/v1/trucks             List trucks (?status)
  GET  /dispatch/api/v1/trucks/:id         Get truck record

  GET  /dispatch/api/v1/trailers           List trailers (?status)
  GET  /dispatch/api/v1/trailers/:id       Get trailer record

  GET   /dispatch/api/v1/facilities         List facilities (?q for name/address substring, ?limit, ?offset)
  GET   /dispatch/api/v1/facilities/:id     Get facility record
  POST  /dispatch/api/v1/facilities         Create facility (body: name, address, contacts?, notes?, tags?, blob_ids?, lat?, lng?). Unknown fields are rejected.
  PATCH /dispatch/api/v1/facilities/:id     Update facility fields. Setting `address` re-queues the geocoder; explicit `lat`+`lng` set status=ready and reset failure count. Unknown fields are rejected.

  GET  /dispatch/api/v1/events             List recent events (?trip_id, ?driver_id, ?limit, ?offset)

## Driver Portal

The driver-facing PWA has its own API namespace at /driver/api/v1/. These endpoints
use JWT auth (not Bearer) and are intended for the driver mobile app only — not for
admin automation. They are not part of the Bearer-protected admin API surface and are
not described in /openapi.json.

Auth endpoints (no auth required):
  POST /driver/api/v1/auth/challenge         Begin WebAuthn assertion or PIN challenge
  POST /driver/api/v1/auth/verify            Complete WebAuthn assertion (returns JWT)
  POST /driver/api/v1/auth/pin               Authenticate with PIN (returns JWT)
  POST /driver/api/v1/auth/register-passkey  Register a new passkey for the authenticated driver
  POST /driver/api/v1/auth/refresh           Refresh an expiring JWT

Data endpoints (JWT required — driver sees only their own trips):
  GET  /driver/api/v1/me                     Driver profile and current status
  GET  /driver/api/v1/trips                  Driver's trips (?tab=current|upcoming|past)
  GET  /driver/api/v1/trips/:id              Trip detail (stops, load summary, equipment)
  GET  /driver/api/v1/trips/:id/stops/:seq   Stop detail (facility contacts, commodity info)
  GET  /driver/api/v1/equipment              Current truck + currently attached trailer(s)
  PUT  /driver/api/v1/equipment/trailer      Set currently attached trailers (body: trailer_ids OR trailer_unit_numbers).
                                             Cascades onto the driver's active Dispatched/InTransit trip's trailer_ids
                                             unless the driver has arrived at the final delivery stop. Emits driver.trailer_changed.
                                             At dispatch time, /trips/:id/dispatch reconciles the trip's trailer_ids to the
                                             driver's current_trailer_ids when they differ.
  GET  /driver/api/v1/trailers               List trailers available for selection (excludes inactive/out_of_service)

## Dispatcher MCP Server

POST /dispatch/mcp — MCP JSON-RPC endpoint for AI agent tool calls. Requires dispatcher JWT (Authorization: Bearer <token> from POST /dispatch/auth/login). Supports tools: list_loads, get_load, create_load, update_load, list_trips, get_trip, assign_driver, unassign_driver, dispatch_trip, undispatch_trip, cancel_trip, complete_trip, stop_arrive, stop_depart, stop_late, check_call, list_drivers, get_driver, list_trucks, list_trailers, list_events.

## Full Spec

Machine-readable OpenAPI 3.0 spec: GET /openapi.json
"#;

async fn llms_txt() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        LLMS_TXT,
    )
}

pub fn router(state: AppState) -> Router {
    let key = state.config.admin_api_key.clone();

    let protected = Router::new()
        // Blobs
        .route("/api/v1/blobs", post(blobs::upload_blob).layer(DefaultBodyLimit::max(50 * 1024 * 1024)))
        .route("/api/v1/blobs", get(blobs::list_blobs))
        .route("/api/v1/blob/:id", get(blob::get_blob))
        .route("/api/v1/blob/:id", put(blob::update_blob))
        .route("/api/v1/blob/:id", delete(blob::delete_blob))
        .route("/api/v1/blobs/:id/query", post(blob::query_blob))
        // Facilities
        .route("/api/v1/facilities", post(facilities::create_facility))
        .route("/api/v1/facilities", get(facilities::list_facilities))
        .route("/api/v1/facilities/:id", get(facilities::get_facility))
        .route("/api/v1/facilities/:id", patch(facilities::update_facility))
        .route("/api/v1/facilities/:id", delete(facilities::delete_facility))
        // Loads — CRUD
        .route("/api/v1/loads", post(loads::create_load))
        .route("/api/v1/loads", get(loads::list_loads))
        .route("/api/v1/loads/:id", get(loads::get_load))
        .route("/api/v1/loads/:id", patch(loads::update_load))
        .route("/api/v1/loads/:id", delete(loads::delete_load))
        // Loads — actions
        .route("/api/v1/loads/:id/invoice", post(loads::invoice_load))
        .route("/api/v1/loads/:id/cancel", post(loads::cancel_load))
        .route("/api/v1/loads/:id/settle", post(loads::settle_load))
        .route("/api/v1/loads/:id/stops/:seq/arrive", post(loads::load_stop_arrive))
        .route("/api/v1/loads/:id/stops/:seq/depart", post(loads::load_stop_depart))
        // Dispatchers
        .merge(dispatchers::router())
        // Drivers, trucks, trailers, trips, trip actions, events (stubs — filled in by Wave 2/3/4)
        .merge(drivers::router())
        .merge(trucks::router())
        .merge(trailers::router())
        .merge(trips::router())
        .merge(trip_actions::router())
        .merge(events::router())
        .route_layer(from_fn(move |req, next| {
            let k = key.clone();
            async move { require_bearer(k, req, next).await }
        }));

    // Dispatcher portal: auth + JWT-protected data endpoints
    let dispatcher_auth = dispatcher_portal::dispatcher_portal_router(&state);

    // Driver portal: auth endpoints + JWT-protected data endpoints (#51 adds routes)
    let driver_portal = driver_portal::portal_router(&state);

    // Static file serving for the driver PWA; SPA fallback to index.html
    let driver_static = tower_http::services::ServeDir::new("static/driver")
        .fallback(tower_http::services::ServeFile::new(
            "static/driver/index.html",
        ));

    // Static file serving for the dispatcher SPA; SPA fallback to index.html
    let dispatch_static = tower_http::services::ServeDir::new("static/dispatch")
        .fallback(tower_http::services::ServeFile::new(
            "static/dispatch/index.html",
        ));

    Router::new()
        .route("/openapi.json", get(openapi_json))
        .route("/llms.txt", get(llms_txt))
        .route("/version", get(version::get_version))
        .merge(protected)
        .merge(dispatcher_auth)
        .merge(driver_portal)
        .nest_service("/driver", driver_static)
        .nest_service("/dispatch", dispatch_static)
        .with_state(state)
}
