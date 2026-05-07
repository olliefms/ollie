// src/api/mod.rs
pub mod auth;
pub mod blob;
pub mod blobs;
pub mod drivers;
pub mod events;
pub mod facilities;
pub mod loads;
pub mod trailers;
pub mod trip_actions;
pub mod trips;
pub mod trucks;

use crate::{api::auth::require_bearer, models, AppState};
use axum::{
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
        loads::load_stop_arrive,
        loads::load_stop_depart,
        events::list_events,
        events::get_event_handler,
        drivers::create_driver,
        drivers::list_drivers,
        drivers::get_driver,
        drivers::update_driver,
        drivers::delete_driver,
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
            loads::LoadStopArriveRequest,
            loads::LoadStopDepartRequest,
        )
    ),
    modifiers(&SecurityAddon),
    info(
        title = "ollie API",
        version = "1.0.0",
        description = "RAG-enabled blob store and freight load management API. \
            All endpoints require Bearer auth except /openapi.json and /llms.txt."
    ),
    tags(
        (name = "blobs", description = "Document blob storage with AI summarisation and semantic search"),
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

All endpoints except /openapi.json and /llms.txt require:
  Authorization: Bearer <ADMIN_API_KEY>

Missing or incorrect key returns 401 Unauthorized.

## Endpoint Groups

### Blobs — /api/v1/blobs, /api/v1/blob/:id
Store and retrieve files (PDFs, images, documents). Files are content-addressed and
deduplicated. Uploaded files are processed asynchronously: Ollama generates a text
summary and a vector embedding. Supports semantic search via ?s=<query>.

  POST   /api/v1/blobs              Upload file (multipart/form-data: file, name?, tags?)
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

Stop fields (all optional): scheduled_arrive_end (window close; null = strict appointment),
actual_arrive, actual_depart, expected_dwell_minutes, detention_free_minutes (default 120),
detention_grace_minutes (default 15). Detention eligibility: FCFS stops (scheduled_arrive_end
set) are eligible if actual_depart > actual_arrive + detention_free_minutes. Strict stops
are eligible only if actual_arrive ≤ scheduled_arrive + grace_minutes (early = on-time).

  POST   /api/v1/loads          Create load
  GET    /api/v1/loads          List or search loads (?s, ?status, ?customer, ?from, ?to, ?tag)
  GET    /api/v1/loads/:id      Get load detail
  PATCH  /api/v1/loads/:id      Update load fields
  DELETE /api/v1/loads/:id      Delete load

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

  POST   /api/v1/trips          Create trip (trip_number auto-generated as T-YYYY-NNNN if omitted)
  GET    /api/v1/trips          List trips (?load_id, ?driver_id, ?status, ?limit, ?offset)
  GET    /api/v1/trips/:id      Get trip record
  PATCH  /api/v1/trips/:id      Update trip fields (load_id, sequence, stops, notes)
  DELETE /api/v1/trips/:id      Soft-delete (transitions to cancelled; blocked if in_transit or delivered)

  POST   /api/v1/trips/:id/assign           Assign driver, truck, trailers (body: driver_id, truck_id, trailer_ids?)
  POST   /api/v1/trips/:id/unassign         Unassign resources and revert to planned
  POST   /api/v1/trips/:id/dispatch         Dispatch trip (must be assigned)
  POST   /api/v1/trips/:id/undispatch       Revert dispatched trip to assigned
  POST   /api/v1/trips/:id/cancel           Cancel trip (blocked if in_transit or delivered)
  POST   /api/v1/trips/:id/stops/:seq/arrive  Record actual arrival at stop (body: actual_arrive)
  POST   /api/v1/trips/:id/stops/:seq/depart  Record actual departure from stop (body: actual_depart); triggers trip/load status cascades
  POST   /api/v1/trips/:id/stops/:seq/late    Flag stop as late (body: eta?, notes?); returns 204
  POST   /api/v1/trips/:id/check-call         Record driver check-in (body: location, notes?, eta_next_stop?); returns 204

### Drivers — /api/v1/drivers, /api/v1/drivers/:id
Driver records with state machine. Status: available → assigned → dispatched (last two driven by trip events).
DELETE soft-deletes (sets status=inactive). PUT cannot set assigned or dispatched.

  POST   /api/v1/drivers          Create driver
  GET    /api/v1/drivers          List or search drivers (?s, ?status, ?limit, ?offset)
  GET    /api/v1/drivers/:id      Get driver
  PUT    /api/v1/drivers/:id      Update driver fields (cannot manually set assigned/dispatched)
  DELETE /api/v1/drivers/:id      Soft-delete (sets status=inactive)

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
        .route("/api/v1/blobs", post(blobs::upload_blob))
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

    Router::new()
        .route("/openapi.json", get(openapi_json))
        .route("/llms.txt", get(llms_txt))
        .merge(protected)
        .with_state(state)
}
