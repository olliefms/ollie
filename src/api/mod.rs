// src/api/mod.rs
pub mod auth;
pub mod blob;
pub mod oauth;
pub mod refresh_tokens;
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
        dispatcher_portal::driver_writes::attach_equipment_handler,
        dispatcher_portal::driver_writes::detach_equipment_handler,
        dispatcher_portal::data::list_trucks,
        dispatcher_portal::data::get_truck,
        dispatcher_portal::truck_writes::create_truck_handler,
        dispatcher_portal::truck_writes::update_truck_handler,
        dispatcher_portal::data::list_trailers,
        dispatcher_portal::data::get_trailer,
        dispatcher_portal::trailer_writes::create_trailer_handler,
        dispatcher_portal::trailer_writes::update_trailer_handler,
        dispatcher_portal::data::list_events,
        dispatcher_portal::blobs::list_blobs,
        dispatcher_portal::blobs::upload_blob,
        dispatcher_portal::blobs::get_blob,
        dispatcher_portal::blobs::update_blob,
        dispatcher_portal::blobs::delete_blob,
        dispatcher_portal::blobs::query_blob,
        dispatcher_portal::blobs::presigned_upload,
        dispatcher_portal::blobs::presigned_download,
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
            dispatcher_portal::truck_writes::CreateTruckBody,
            dispatcher_portal::truck_writes::PatchTruckBody,
            dispatcher_portal::trailer_writes::CreateTrailerBody,
            dispatcher_portal::trailer_writes::PatchTrailerBody,
            dispatcher_portal::driver_writes::AttachEquipmentBody,
            dispatcher_portal::driver_writes::DetachEquipmentBody,
            dispatcher_portal::driver_writes::DriverEquipmentChange,
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

ollie is a freight load-management system with RAG-enabled document storage. The
core resources are loads, trips, drivers, trucks, trailers, facilities, and
document blobs. Uploaded documents are summarized and embedded by Ollama for
semantic search.

## Which surface should I use?

ollie exposes four surfaces. Choose by caller, not by habit:

  Dispatcher MCP    POST /dispatch/mcp     AI agents and tool-using assistants. PREFERRED.
  Dispatcher REST   /dispatch/api/v1/*     Dispatcher web app and programmatic integrations.
  Driver portal     /driver/api/v1/*       The driver mobile PWA only.
  Admin REST        /api/v1/*              DEPRECATED — see "Admin REST (deprecated)" below.

New automation should target the dispatcher MCP server, falling back to the
dispatcher REST API where no tool exists for an operation. The admin REST surface
is retained for backward compatibility only and will be removed in a future release.

## Authentication

  Dispatcher MCP / REST   Authorization: Bearer <JWT>          (POST /dispatch/auth/login with email+password, or a dispatcher API key)
  Driver portal           Authorization: Bearer <JWT>          (driver passkey or PIN auth)
  Admin REST              Authorization: Bearer <ADMIN_API_KEY>  (deprecated)

Public, no auth: GET /version, GET /openapi.json, GET /llms.txt.
Missing or incorrect credentials return 401. Dispatcher login locks out after 5
failed attempts (15 min × 2^(failures-5), capped at 24h; 423 with locked_until).

Dispatcher API keys (for headless/programmatic callers — used in the Authorization
header just like a JWT) are managed under /dispatch/api-keys (a password/JWT session
is required; an API-key session cannot mint more keys):
  POST   /dispatch/api-keys      Create a key. Body: { label (1-64 chars), expires_in_days? (1-365, default 365) }.
                                 Returns the plaintext `key` exactly once — it is never retrievable again. Max 20 active keys per dispatcher.
  GET    /dispatch/api-keys      List the caller's active keys (label, key_prefix, created/expires/last_used; no plaintext).
  DELETE /dispatch/api-keys/:id  Revoke a key.

## Dispatcher MCP server — POST /dispatch/mcp

JSON-RPC 2.0. Call `tools/list` for input schemas, `tools/call` to invoke. Requires
a dispatcher JWT or API key in the Authorization header.

Loads & trips:
  list_loads, get_load, create_load, update_load
  list_trips, get_trip, create_trip, update_trip, recalculate_trip_miles
  assign_driver, unassign_driver, dispatch_trip, undispatch_trip, cancel_trip, complete_trip
  stop_arrive, stop_depart, stop_late, check_call

Fleet & facilities:
  list_drivers, get_driver, attach_equipment, detach_equipment
  list_trucks, get_truck, create_truck, update_truck
  list_trailers, get_trailer, create_trailer, update_trailer
  list_facilities, get_facility, create_facility, update_facility
  list_events

Data-integrity doctors:
  trip_doctor, load_doctor, facility_doctor — diagnose (and optionally repair) one record.

Document blobs:
  upload_blob        Returns a short-lived presigned POST URL. POST the raw file bytes to
                     that URL (Content-Type header; optional ?name=&tags=); the HTTP response
                     is the created blob record, whose id you pass in blob_ids. File bytes
                     never pass through the MCP call — do NOT base64 a document into a tool
                     argument.
  get_blob_url       Presigned GET URL for a blob's bytes. Stream large files to disk.
  get_blob_metadata  Blob metadata plus a reverse lookup:
                     attached_to.{loads,facilities,trips,drivers,trucks,trailers}.
  list_blobs         List blob metadata (optional name, tag, content_type, limit).
  delete_blob        Delete a blob (force? to override reference checks).

Presigned URLs require OLLIE_PUBLIC_BASE_URL to be configured on the server.

### Presigned blob byte-transfer — /dispatch/blobs/presigned

The endpoints the blob tools hand out. Token-authenticated via ?token= (no JWT
header), mounted outside the dispatcher middleware so a credential-less agent can
use a minted URL directly. Each token is bound to one operation (and, for GET, one
blob id) and expires (default 300s).

  POST /dispatch/blobs/presigned?token=…       Upload raw body bytes (Content-Type header; optional ?name=&tags=). 50 MB limit. Returns the blob record.
  GET  /dispatch/blobs/presigned/{id}?token=…  Download raw bytes.

## Dispatcher REST — /dispatch/api/v1

JWT auth; same response shapes as the resources above. Use when a needed operation
has no MCP tool. Auth lives at /dispatch/auth/ (POST /login, POST /refresh — refresh
within the 7-day window).

  Loads      GET /loads, GET /loads/:id, POST /loads, PUT /loads/:id
  Trips      GET /trips, GET /trips/:id,
             POST /trips/:id/{assign,unassign,dispatch,undispatch,cancel,complete},
             POST /trips/:id/stops/:seq/{arrive,depart,late}, POST /trips/:id/check-call
  Drivers    GET /drivers, GET /drivers/:id,
             POST /drivers/:id/attach-equipment, POST /drivers/:id/detach-equipment
  Trucks     GET /trucks, GET /trucks/:id, POST /trucks, PATCH /trucks/:id
  Trailers   GET /trailers, GET /trailers/:id, POST /trailers, PATCH /trailers/:id
  Facilities GET /facilities (?q, ?limit, ?offset), GET /facilities/:id, POST /facilities, PATCH /facilities/:id
  Blobs      GET /blobs, GET /blob/:id, POST /blobs, PUT /blob/:id, DELETE /blob/:id, POST /blobs/:id/query
             (multipart POST /blobs accepts an optional visibility=driver field to expose the
             document in the driver portal; prefer the presigned flow above for large uploads)
  Events     GET /events (?trip_id, ?driver_id, ?limit, ?offset)
  Counts     GET /loads/count, /drivers/count, /blobs/count, /events/count

Truck/trailer PATCH: `status` is not settable — equipment transitions via the trip
lifecycle; unknown body fields are rejected. Facility PATCH: setting `address`
re-queues the geocoder, while explicit lat+lng set geocode_status=ready and reset
the failure count.

## Domain model

### Load lifecycle
  planned → assigned → dispatched → in_transit → delivered → invoiced → settled
  Cancel is allowed from planned, assigned, dispatched, or in_transit.
  Creating a trip with both load_id and driver_id auto-assigns a planned load.

### Trip lifecycle
  planned → assigned → dispatched → in_transit → delivered
  Assign/dispatch are reversible; cancel is allowed from planned, assigned, or
  dispatched only (in_transit and delivered are terminal — use a relay trip instead).
  A load may have multiple trips (relay). Trip responses include: previous_trip_id
  (auto-chained to the driver's last non-cancelled trip unless provided),
  deadhead_miles and loaded_miles (ORS HGV routing; null when facilities lack
  coordinates), load_number (denormalized at creation), and per-stop address (from
  the linked facility at creation). When `stops` is omitted or empty and `load_id`
  is set, stops are inherited from the load; pass an explicit `stops` array to override.

### Stops, times, and detention
  A stop needs scheduled_arrive (naive local datetime, e.g. "2026-05-10T08:00:00")
  AND timezone (IANA, e.g. "America/Chicago") together — one without the other is 422.
  Times are stored naive; timezone is the authoritative offset, so a Z/offset suffix
  is rejected when a timezone is set. Legacy stops (pre-v1.3.3) carry timezone:null
  with UTC times and are not silently converted.
  Optional: scheduled_arrive_end (window close; null = strict appointment),
  actual_arrive, actual_depart, expected_dwell_minutes, detention_free_minutes
  (default 120), detention_grace_minutes (default 15). Detention: FCFS stops
  (scheduled_arrive_end set) accrue when actual_depart > actual_arrive +
  detention_free_minutes; strict stops are eligible only when actual_arrive ≤
  scheduled_arrive + grace_minutes (early counts as on-time).

### Facility resolution
  On load create/update a stop may give facility_id, or name + address. Ambiguous
  name+address matches return 200 with an array of FacilityResolutionResponse objects
  (one per ambiguous stop, each with a stop_index). Retry with facility_id set, or
  force_new_facility=true to create a new facility for that stop.

### Document blobs
  Files are content-addressed and deduplicated — identical bytes share storage and AI
  output. Each upload is processed asynchronously: Ollama generates a text summary and
  a vector embedding (status: pending → processing → ready | failed). Semantic search
  via ?s=<query>. Ask a natural-language question about a ready document via
  POST /dispatch/api/v1/blobs/:id/query (body: { prompt, model? }).

### List vs. search counts
  GET list endpoints return a `returned` field. Without ?s= it is the total matching
  count (for pagination); with ?s=<query> it is the number of items in this response.

## Driver portal — /driver/api/v1 (driver app only)

JWT auth (passkey or PIN); a driver sees only their own trips. Not part of the admin
surface and not described in /openapi.json.

  Auth:  POST /auth/{challenge,verify,pin,register-passkey,refresh}
  Data:  GET /me, GET /trips (?tab=current|upcoming|past), GET /trips/:id,
         GET /trips/:id/stops/:seq, GET /equipment, PUT /equipment/trailer, GET /trailers
  PUT /equipment/trailer sets the driver's currently attached trailers (body:
  trailer_ids OR trailer_unit_numbers) and cascades onto their active
  Dispatched/InTransit trip unless they have arrived at the final delivery stop.

## Admin REST — /api/v1 (DEPRECATED)

Retained for backward compatibility and slated for removal — new integrations must use
the dispatcher surface instead. The admin API mirrors the resources above under
/api/v1/* with Authorization: Bearer <ADMIN_API_KEY>, plus a few admin-only endpoints:

  Dispatchers  POST/GET/PUT /api/v1/dispatchers[…], PUT /api/v1/dispatchers/:id/password
  Drivers      POST/PUT/DELETE /api/v1/drivers[…], POST /api/v1/drivers/:id/pin
  Loads, trips, trucks, trailers, facilities, blobs, events — full CRUD and lifecycle,
    same shapes as the dispatcher surface.

See /openapi.json for the complete admin endpoint reference.

## Full spec

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
        .route("/api/v1/blob/{id}", get(blob::get_blob))
        .route("/api/v1/blob/{id}", put(blob::update_blob))
        .route("/api/v1/blob/{id}", delete(blob::delete_blob))
        .route("/api/v1/blobs/{id}/query", post(blob::query_blob))
        // Facilities
        .route("/api/v1/facilities", post(facilities::create_facility))
        .route("/api/v1/facilities", get(facilities::list_facilities))
        .route("/api/v1/facilities/{id}", get(facilities::get_facility))
        .route("/api/v1/facilities/{id}", patch(facilities::update_facility))
        .route("/api/v1/facilities/{id}", delete(facilities::delete_facility))
        // Loads — CRUD
        .route("/api/v1/loads", post(loads::create_load))
        .route("/api/v1/loads", get(loads::list_loads))
        .route("/api/v1/loads/{id}", get(loads::get_load))
        .route("/api/v1/loads/{id}", patch(loads::update_load))
        .route("/api/v1/loads/{id}", delete(loads::delete_load))
        // Loads — actions
        .route("/api/v1/loads/{id}/invoice", post(loads::invoice_load))
        .route("/api/v1/loads/{id}/cancel", post(loads::cancel_load))
        .route("/api/v1/loads/{id}/settle", post(loads::settle_load))
        .route("/api/v1/loads/{id}/stops/{seq}/arrive", post(loads::load_stop_arrive))
        .route("/api/v1/loads/{id}/stops/{seq}/depart", post(loads::load_stop_depart))
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

    // Presigned blob byte-transfer routes — token-authenticated, no JWT middleware
    let dispatcher_public = dispatcher_portal::public_router();

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
        .merge(dispatcher_public)
        .merge(driver_portal)
        .merge(oauth::router())
        .nest_service("/driver", driver_static)
        .nest_service("/dispatch", dispatch_static)
        .with_state(state)
}
