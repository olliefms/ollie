// src/api/mod.rs
pub mod blob;
pub mod oauth;
pub mod refresh_tokens;
pub mod utils;
pub mod version;
pub mod blobs;
pub mod fleet_portal;
pub mod driver_portal;
pub mod drivers;
pub mod facilities;
pub mod loads;
pub mod mileage_summary;
pub mod trailers;
pub mod trips;
pub mod trucks;

use crate::{models, AppState};
use axum::{
    response::IntoResponse,
    routing::get,
    Router,
};
use utoipa::OpenApi;
use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};

#[derive(OpenApi)]
#[openapi(
    paths(
        fleet_portal::auth::login,
        fleet_portal::auth::refresh,
        fleet_portal::auth::setup_status,
        fleet_portal::auth::setup,
        fleet_portal::data::list_loads,
        fleet_portal::data::get_load,
        fleet_portal::data::create_load,
        fleet_portal::data::update_load,
        fleet_portal::data::delete_load_handler,
        fleet_portal::data::invoice_load_handler,
        fleet_portal::data::cancel_load_handler,
        fleet_portal::data::settle_load_handler,
        fleet_portal::data::list_trips,
        fleet_portal::data::create_trip_handler,
        fleet_portal::data::get_trip,
        fleet_portal::data::assign_trip,
        fleet_portal::data::unassign_trip,
        fleet_portal::data::dispatch_trip,
        fleet_portal::data::undispatch_trip,
        fleet_portal::data::cancel_trip,
        fleet_portal::data::complete_trip,
        fleet_portal::data::stop_arrive,
        fleet_portal::data::stop_depart,
        fleet_portal::data::stop_late,
        fleet_portal::data::check_call,
        fleet_portal::trip_writes::recalculate_miles_handler,
        fleet_portal::trip_writes::patch_trip_handler,
        fleet_portal::trip_writes::delete_trip_handler,
        fleet_portal::data::list_facilities,
        fleet_portal::data::get_facility,
        fleet_portal::facility_writes::create_facility_handler,
        fleet_portal::facility_writes::update_facility_handler,
        fleet_portal::facility_writes::archive_facility_handler,
        fleet_portal::facility_writes::reactivate_facility_handler,
        fleet_portal::facility_writes::permanent_delete_facility_handler,
        fleet_portal::data::list_drivers,
        fleet_portal::data::get_driver,
        fleet_portal::driver_writes::create_driver_handler,
        fleet_portal::driver_writes::patch_driver_handler,
        fleet_portal::driver_writes::delete_driver_handler,
        fleet_portal::driver_writes::set_driver_pin_handler,
        fleet_portal::driver_writes::attach_equipment_handler,
        fleet_portal::driver_writes::detach_equipment_handler,
        fleet_portal::terminal_writes::create_terminal,
        fleet_portal::terminal_writes::list_terminals,
        fleet_portal::terminal_writes::get_terminal,
        fleet_portal::terminal_writes::update_terminal,
        fleet_portal::terminal_writes::delete_terminal,
        fleet_portal::data::list_trucks,
        fleet_portal::data::get_truck,
        fleet_portal::truck_writes::create_truck_handler,
        fleet_portal::truck_writes::update_truck_handler,
        fleet_portal::truck_writes::delete_truck_handler,
        fleet_portal::data::list_trailers,
        fleet_portal::data::get_trailer,
        fleet_portal::trailer_writes::create_trailer_handler,
        fleet_portal::trailer_writes::update_trailer_handler,
        fleet_portal::trailer_writes::delete_trailer_handler,
        fleet_portal::data::list_maintenance,
        fleet_portal::data::get_maintenance,
        fleet_portal::maintenance_writes::create_maintenance_handler,
        fleet_portal::maintenance_writes::update_maintenance_handler,
        fleet_portal::maintenance_writes::delete_maintenance_handler,
        fleet_portal::data::list_events,
        fleet_portal::expenses::create_expense_handler,
        fleet_portal::expenses::list_expenses_handler,
        fleet_portal::expenses::get_expense_handler,
        fleet_portal::expenses::review_expense_handler,
        fleet_portal::expenses::patch_expense_handler,
        fleet_portal::expenses::delete_expense_handler,
        fleet_portal::users::me,
        fleet_portal::users::list_users,
        fleet_portal::users::get_user,
        fleet_portal::users::create_user,
        fleet_portal::users::update_user,
        fleet_portal::users::reset_user_password,
        fleet_portal::users::delete_user,
        fleet_portal::blobs::list_blobs,
        fleet_portal::blobs::upload_blob,
        fleet_portal::blobs::get_blob,
        fleet_portal::blobs::update_blob,
        fleet_portal::blobs::delete_blob,
        fleet_portal::blobs::query_blob,
        fleet_portal::blobs::presigned_upload,
        fleet_portal::blobs::presigned_download,
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
            models::EquipmentType,
            models::MaintenanceCategory,
            models::MaintenanceRecord,
            models::MaintenanceListItem,
            models::MaintenanceListResponse,
            fleet_portal::maintenance_writes::CreateMaintenanceBody,
            fleet_portal::maintenance_writes::PatchMaintenanceBody,
            models::ExpenseCategory,
            models::ExpenseStatus,
            models::PaymentMethod,
            models::ExpenseRecord,
            models::ExpenseResponse,
            models::ExpenseListResponse,
            fleet_portal::expenses::CreateExpenseBody,
            fleet_portal::expenses::ReviewExpenseBody,
            fleet_portal::expenses::PatchExpenseBody,
            models::TripStatus,
            models::TripStopType,
            models::TripStop,
            models::TripRecord,
            models::CreateTripRequest,
            models::UpdateTripRequest,
            models::TripListItem,
            models::TripListResponse,
            crate::services::trip_lifecycle::AssignTripRequest,
            crate::services::trip_lifecycle::StopArriveRequest,
            crate::services::trip_lifecycle::StopDepartRequest,
            crate::services::trip_lifecycle::StopLateRequest,
            crate::services::trip_lifecycle::CheckCallRequest,
            fleet_portal::trip_writes::RecalculateMilesBody,
            fleet_portal::trip_writes::PatchTripBody,
            fleet_portal::trip_writes::PatchTripResult,
            fleet_portal::facility_writes::CreateFacilityBody,
            fleet_portal::facility_writes::PatchFacilityBody,
            fleet_portal::truck_writes::CreateTruckBody,
            fleet_portal::truck_writes::PatchTruckBody,
            fleet_portal::trailer_writes::CreateTrailerBody,
            fleet_portal::trailer_writes::PatchTrailerBody,
            fleet_portal::driver_writes::AttachEquipmentBody,
            fleet_portal::driver_writes::DetachEquipmentBody,
            fleet_portal::driver_writes::DriverEquipmentChange,
            fleet_portal::data::FleetTripListItem,
            models::terminal::TerminalRecord,
            models::terminal::TerminalListItem,
            models::terminal::CreateTerminalRequest,
            models::terminal::UpdateTerminalRequest,
            models::pay::DriverPay,
            driver_portal::data::DriverFacilityContact,
            driver_portal::data::UpdateStopTimesRequest,
            driver_portal::equipment::EquipmentTruckSummary,
            driver_portal::equipment::EquipmentTrailerSummary,
            driver_portal::equipment::DriverEquipmentResponse,
            driver_portal::equipment::UpdateTrailerRequest,
            driver_portal::equipment::UpdateTrailerResponse,
            driver_portal::equipment::AvailableTrailerItem,
            driver_portal::equipment::AvailableTrailersResponse,
            models::FleetUserStatus,
            models::FleetUserRecord,
            models::Role,
            fleet_portal::users::MeResponse,
            fleet_portal::users::CreateUserRequest,
            fleet_portal::users::UpdateUserRequest,
            fleet_portal::users::ResetUserPasswordRequest,
            fleet_portal::users::UserListResponse,
            fleet_portal::auth::LoginRequest,
            fleet_portal::auth::LoginResponse,
            fleet_portal::auth::LockResponse,
            fleet_portal::auth::SetupRequest,
            fleet_portal::auth::SetupStatusResponse,
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
        (name = "fleet", description = "Fleet portal data API — loads, trips, drivers, trucks, trailers, maintenance, events"),
        (name = "fleet-auth", description = "Fleet portal authentication — login and JWT refresh"),
        (name = "fleet_users", description = "Fleet user admin CRUD and password management"),
        (name = "drivers", description = "Driver management with state machine"),
        (name = "events", description = "Append-only event journal (read-only)"),
        (name = "facilities", description = "Freight facility management with geocoding and semantic search"),
        (name = "loads", description = "Freight load lifecycle management"),
        (name = "maintenance", description = "Equipment maintenance log tied to a truck or trailer"),
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
core resources are loads, trips, drivers, trucks, trailers, facilities,
maintenance records, and document blobs. Uploaded documents are summarized and embedded by Ollama for
semantic search.

## Which surface should I use?

ollie exposes three surfaces. Choose by caller, not by habit:

  Fleet MCP    POST /fleet/mcp     AI agents and tool-using assistants. PREFERRED.
  Fleet REST   /fleet/api/v1/*     Fleet web app and programmatic integrations.
  Driver portal     /driver/api/v1/*       The driver mobile PWA only.

New automation should target the fleet MCP server, falling back to the
fleet REST API where no tool exists for an operation.

## Authentication

  Fleet MCP / REST        Authorization: Bearer <JWT>          (POST /fleet/auth/login with email+password, or a fleet_user API key)
  Driver portal           Authorization: Bearer <JWT>          (driver passkey or PIN auth)

Public, no auth: GET /version, GET /openapi.json, GET /llms.txt.
Missing or incorrect credentials return 401. Fleet login locks out after 5
failed attempts (15 min × 2^(failures-5), capped at 24h; 423 with locked_until).

Fleet user API keys (for headless/programmatic callers — used in the Authorization
header just like a JWT) are managed under /fleet/api-keys (a password/JWT session
is required; an API-key session cannot mint more keys):
  POST   /fleet/api-keys      Create a key. Body: { label (1-64 chars), expires_in_days? (1-365, default 365) }.
                                 Returns the plaintext `key` exactly once — it is never retrievable again. Max 20 active keys per fleet_user.
  GET    /fleet/api-keys      List the caller's active keys (label, key_prefix, created/expires/last_used; no plaintext).
  DELETE /fleet/api-keys/:id  Revoke a key.

## Fleet MCP server — POST /fleet/mcp

MCP Streamable HTTP transport (protocol 2025-06-18), JSON-RPC 2.0. Requires a
fleet user JWT or API key in the Authorization header. Every POST must send
`Accept: application/json, text/event-stream` and `Content-Type: application/json`;
responses stream back as `text/event-stream`.

Lifecycle: POST `initialize` first — the response carries an `Mcp-Session-Id`
header; send it back as the `Mcp-Session-Id` request header on every subsequent
call. Then call `tools/list` for input schemas and `tools/call` to invoke. A stock
MCP client (e.g. an `type: "http"` config) handles all of this for you.

Loads & trips:
  list_loads, get_load, create_load, update_load
  list_trips, get_trip, create_trip, update_trip, recalculate_trip_miles
  assign_driver, unassign_driver, dispatch_trip, undispatch_trip, cancel_trip, complete_trip
  stop_arrive, stop_depart, stop_late, check_call

Fleet & facilities:
  list_drivers, get_driver, attach_equipment, detach_equipment
  list_trucks, get_truck, create_truck, update_truck
  list_trailers, get_trailer, create_trailer, update_trailer
  list_maintenance, get_maintenance, create_maintenance, update_maintenance
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
                     attached_to.{loads,facilities,trips,drivers,trucks,trailers,maintenance}.
  list_blobs         List blob metadata (optional name, tag, content_type, limit;
                     missing_summary=true returns only blobs with no AI summary).
  update_blob        Edit a blob's name, tags, and/or AI summary (tags is a full replace
                     of the set; a summary is re-embedded and marks the blob ready —
                     the backfill path for docs the pipeline couldn't summarize).
  resummarize_blob   Re-queue one blob through the AI pipeline (extract → summarize →
                     embed). Asynchronous — poll get_blob_metadata for status ready.
  delete_blob        Delete a blob (force? to override reference checks).

Presigned URLs require OLLIE_PUBLIC_BASE_URL to be configured on the server.

### Presigned blob byte-transfer — /fleet/blobs/presigned

The endpoints the blob tools hand out. Token-authenticated via ?token= (no JWT
header), mounted outside the fleet user middleware so a credential-less agent can
use a minted URL directly. Each token is bound to one operation (and, for GET, one
blob id) and expires (default 300s).

  POST /fleet/blobs/presigned?token=…       Upload raw body bytes (Content-Type header; optional ?name=&tags=). 50 MB limit. Returns the blob record.
  GET  /fleet/blobs/presigned/{id}?token=…  Download raw bytes.

## Fleet REST — /fleet/api/v1

JWT auth; same response shapes as the resources above. Use when a needed operation
has no MCP tool. Auth lives at /fleet/auth/ (POST /login, POST /refresh — refresh
within the 7-day window).

  Loads      GET /loads, GET /loads/:id, POST /loads, PUT /loads/:id, DELETE /loads/:id,
             POST /loads/:id/{invoice,cancel,settle}
  Trips      GET /trips, GET /trips/:id, POST /trips, DELETE /trips/:id,
             POST /trips/:id/{assign,unassign,dispatch,undispatch,cancel,complete},
             POST /trips/:id/stops/:seq/{arrive,depart,late}, POST /trips/:id/check-call
  Drivers    GET /drivers, GET /drivers/:id, POST /drivers, PATCH /drivers/:id, DELETE /drivers/:id,
             POST /drivers/:id/pin, POST /drivers/:id/attach-equipment, POST /drivers/:id/detach-equipment
  Trucks     GET /trucks, GET /trucks/:id, POST /trucks, PATCH /trucks/:id, DELETE /trucks/:id
  Trailers   GET /trailers, GET /trailers/:id, POST /trailers, PATCH /trailers/:id, DELETE /trailers/:id
  Maintenance GET /maintenance (?equipment_type, ?equipment_id, ?category), GET /maintenance/:id,
             POST /maintenance, PATCH /maintenance/:id, DELETE /maintenance/:id
             (scope: maintenance:read / maintenance:write / maintenance:delete)
  Facilities GET /facilities (?q, ?limit, ?offset), GET /facilities/:id, POST /facilities, PATCH /facilities/:id
  Blobs      GET /blobs, GET /blob/:id, POST /blobs, PUT /blob/:id, DELETE /blob/:id, POST /blobs/:id/query
             (multipart POST /blobs accepts an optional visibility=driver field to expose the
             document in the driver portal; prefer the presigned flow above for large uploads)
  Events     GET /events (?trip_id, ?driver_id, ?limit, ?offset)
  Users      GET /users, GET /users/:id, POST /users, PATCH /users/:id,
             PUT /users/:id/password, DELETE /users/:id
  Counts     GET /loads/count, /drivers/count, /blobs/count, /events/count

Truck/trailer PATCH: `status` is not settable — equipment transitions via the trip
lifecycle; unknown body fields are rejected. Facility PATCH: setting `address`
re-queues the geocoder, while explicit lat+lng set geocode_status=ready and reset
the failure count.

### Fleet users & roles — /fleet/api/v1/users (requires users:* scopes)
The Users surface manages fleet user accounts (roles: owner, fleet_manager,
fleet_user) and per-user extra_scopes. It is gated by `users:*` scopes, which only
owner and fleet_manager hold — a plain fleet_user is forbidden (403) from every
endpoint. Responses expose the user record (id, email, name, status, role,
extra_scopes, timestamps) and NEVER password hashes. MCP parity tools: list_users,
get_user, create_user, update_user, reset_user_password, delete_user.
  POST   /users               Create {email, name, password, role, extra_scopes?}. role=owner rejected.
  PATCH  /users/:id           Update {name?, status?, role?, extra_scopes?}.
  PUT    /users/:id/password  Reset {password} (bumps token_version, invalidates JWTs).
  DELETE /users/:id           Deactivate (status→inactive + bump token_version).
Owner-protection: at least one active owner must always exist; the owner cannot be
demoted or deactivated except via ownership transfer. Ownership transfer (owner-only):
a PATCH setting a different user's role to owner promotes that user and demotes the
calling owner to fleet_manager. A fleet_manager attempting to set role=owner is
forbidden (403).

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
  a vector embedding (status: pending → processing → ready | failed). Scanned/image-only
  PDFs are summarized via the vision model (the embedded page image is recovered and
  described); GET /blobs?missing_summary=true lists docs the pipeline couldn't summarize.
  Semantic search via ?s=<query>. Ask a natural-language question about a ready document
  via POST /fleet/api/v1/blobs/:id/query (body: { prompt, model? }).

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
    // Fleet portal: auth + JWT-protected data endpoints
    let fleet_user_auth = fleet_portal::fleet_portal_router(&state);

    // Presigned blob byte-transfer routes — token-authenticated, no JWT middleware
    let fleet_user_public = fleet_portal::public_router();

    // Driver portal: auth endpoints + JWT-protected data endpoints (#51 adds routes)
    let driver_portal = driver_portal::portal_router(&state);

    // Static file serving for the driver PWA; SPA fallback to index.html
    let driver_static = tower_http::services::ServeDir::new("static/driver")
        .fallback(tower_http::services::ServeFile::new(
            "static/driver/index.html",
        ));

    // Static file serving for the fleet SPA; SPA fallback to index.html
    let fleet_static = tower_http::services::ServeDir::new("static/fleet")
        .fallback(tower_http::services::ServeFile::new(
            "static/fleet/index.html",
        ));

    Router::new()
        .route("/openapi.json", get(openapi_json))
        .route("/llms.txt", get(llms_txt))
        .route("/version", get(version::get_version))
        .merge(fleet_user_auth)
        .merge(fleet_user_public)
        .merge(driver_portal)
        .merge(oauth::router())
        .nest_service("/driver", driver_static)
        .nest_service("/fleet", fleet_static)
        .with_state(state)
}
