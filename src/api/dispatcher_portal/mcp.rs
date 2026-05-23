// src/api/dispatcher_portal/mcp.rs
//
// MCP transport: hand-rolled JSON-RPC 2.0 over HTTP POST.
//
// rmcp (official Rust MCP SDK) was evaluated but targets Axum 0.8, which
// conflicts with this project's Axum 0.7 dependency. Rather than upgrading
// Axum (a large breaking change mid-release), we hand-roll the thin JSON-RPC
// envelope (~150 lines). Tool handlers are thin shims into existing DB ops;
// no business logic lives here. Switch to rmcp once the project upgrades to
// Axum 0.8.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    api::trip_actions::{
        self, CheckCallRequest, StopArriveRequest, StopDepartRequest, StopLateRequest,
    },
    events,
    models::{DriverStatus, LoadStatus, TrailerStatus, TripStatus, TruckStatus},
    AppState,
};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 envelope types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self { jsonrpc: "2.0".into(), id, result: Some(result), error: None }
    }

    fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError { code, message: message.into() }),
        }
    }
}

/// Wrap a serializable value in the MCP content format.
fn mcp_content(value: impl Serialize) -> Value {
    let text = serde_json::to_string(&value).unwrap_or_default();
    json!({ "content": [{ "type": "text", "text": text }] })
}

// ---------------------------------------------------------------------------
// tools/list schema
// ---------------------------------------------------------------------------

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "list_loads",
                "description": "List loads. Optional filters: status (planned/assigned/dispatched/in_transit/delivered/invoiced/settled/cancelled), facility_id (UUID).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "status": { "type": "string", "enum": ["planned","assigned","dispatched","in_transit","delivered","invoiced","settled","cancelled"] },
                        "facility_id": { "type": "string", "format": "uuid" }
                    }
                }
            },
            {
                "name": "get_load",
                "description": "Get a single load by UUID.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "format": "uuid" }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "create_load",
                "description": "Create a new freight load.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "customer_name": { "type": "string" },
                        "customer_ref": { "type": "string" },
                        "stops": { "type": "array", "items": { "type": "object" } },
                        "rate_items": { "type": "array", "items": { "type": "object" } },
                        "commodity": { "type": "string" },
                        "weight_lbs": { "type": "number" },
                        "miles": { "type": "number" },
                        "notes": { "type": "string" },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "blob_ids": { "type": "array", "items": { "type": "string", "format": "uuid" } },
                        "load_number": { "type": "integer" }
                    },
                    "required": ["customer_name", "stops"]
                }
            },
            {
                "name": "update_load",
                "description": "Update fields on an existing load.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "customer_name": { "type": "string" },
                        "customer_ref": { "type": "string" },
                        "stops": { "type": "array", "items": { "type": "object" } },
                        "rate_items": { "type": "array", "items": { "type": "object" } },
                        "commodity": { "type": "string" },
                        "weight_lbs": { "type": "number" },
                        "miles": { "type": "number" },
                        "notes": { "type": "string" },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "blob_ids": { "type": "array", "items": { "type": "string", "format": "uuid" } }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "list_trips",
                "description": "List trips. Items carry deadhead_miles, loaded_miles, total_miles, and origin_facility_name for fleet-wide audits without N+1 get_trip calls. Optional filters: load_id, driver_id, status, trip_number, load_number.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "load_id": { "type": "string", "format": "uuid" },
                        "driver_id": { "type": "string", "format": "uuid" },
                        "status": { "type": "string" },
                        "trip_number": { "type": "string", "description": "Exact match, e.g. 'T-2026-0014'" },
                        "load_number": { "type": "string", "description": "Filter to trips of a load by its load_number (e.g. 'LD-2026-0001')" }
                    }
                }
            },
            {
                "name": "get_trip",
                "description": "Get a single trip by UUID. Response includes a full mileage_summary (origin block + per-leg breakdown).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "format": "uuid" }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "create_trip",
                "description": "Create a new trip. If load_id is given, stops can be omitted and will be copied from the load.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_number": { "type": "string" },
                        "load_id": { "type": "string", "format": "uuid" },
                        "sequence": { "type": "integer" },
                        "driver_id": { "type": "string", "format": "uuid" },
                        "truck_id": { "type": "string", "format": "uuid" },
                        "trailer_ids": { "type": "array", "items": { "type": "string", "format": "uuid" } },
                        "stops": { "type": "array", "items": { "type": "object" } },
                        "notes": { "type": "string" },
                        "previous_trip_id": { "type": "string", "format": "uuid" }
                    }
                }
            },
            {
                "name": "update_trip",
                "description": "Update a trip's notes and/or previous_trip_id link. Setting previous_trip_id triggers a mileage recompute. Mileage fields cannot be set directly.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" },
                        "notes": { "type": "string" },
                        "previous_trip_id": { "type": "string", "format": "uuid" }
                    },
                    "required": ["trip_id"]
                }
            },
            {
                "name": "recalculate_trip_miles",
                "description": "Recompute deadhead/loaded/total miles for a trip via ORS routing. Returns the updated mileage_summary. Use force=true to recompute even when miles are already set.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" },
                        "force": { "type": "boolean" }
                    },
                    "required": ["trip_id"]
                }
            },
            {
                "name": "assign_driver",
                "description": "Assign a driver, truck, and trailers to a trip.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" },
                        "driver_id": { "type": "string", "format": "uuid" },
                        "truck_id": { "type": "string", "format": "uuid" },
                        "trailer_ids": { "type": "array", "items": { "type": "string", "format": "uuid" } }
                    },
                    "required": ["trip_id", "driver_id", "truck_id"]
                }
            },
            {
                "name": "unassign_driver",
                "description": "Unassign the driver and equipment from a trip.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" }
                    },
                    "required": ["trip_id"]
                }
            },
            {
                "name": "dispatch_trip",
                "description": "Dispatch a trip (assigned → dispatched). Trip must be in assigned status; driver/truck must not already be dispatched elsewhere.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "trip_id": { "type": "string", "format": "uuid" } },
                    "required": ["trip_id"]
                }
            },
            {
                "name": "undispatch_trip",
                "description": "Revert a dispatched trip back to assigned. Trip must be in dispatched status (not in_transit or beyond).",
                "inputSchema": {
                    "type": "object",
                    "properties": { "trip_id": { "type": "string", "format": "uuid" } },
                    "required": ["trip_id"]
                }
            },
            {
                "name": "cancel_trip",
                "description": "Cancel a trip. Blocked if the trip is in_transit or delivered.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "trip_id": { "type": "string", "format": "uuid" } },
                    "required": ["trip_id"]
                }
            },
            {
                "name": "complete_trip",
                "description": "Complete a delivered trip and release the driver, truck, and trailers back to available. Trip must be in delivered status.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "trip_id": { "type": "string", "format": "uuid" } },
                    "required": ["trip_id"]
                }
            },
            {
                "name": "stop_arrive",
                "description": "Record actual arrival at a trip stop. Cascades the actual_arrive to the linked load stop when present.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" },
                        "sequence": { "type": "integer", "minimum": 1 },
                        "actual_arrive": { "type": "string", "description": "Naive local datetime when the stop has a timezone (e.g. 2026-05-10T08:00:00)" }
                    },
                    "required": ["trip_id", "sequence", "actual_arrive"]
                }
            },
            {
                "name": "stop_depart",
                "description": "Record actual departure from a trip stop. Triggers trip and load status cascades (e.g. dispatched → in_transit on pickup, → delivered on final stop).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" },
                        "sequence": { "type": "integer", "minimum": 1 },
                        "actual_depart": { "type": "string" }
                    },
                    "required": ["trip_id", "sequence", "actual_depart"]
                }
            },
            {
                "name": "stop_late",
                "description": "Flag a trip stop as late with an optional ETA and notes.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" },
                        "sequence": { "type": "integer", "minimum": 1 },
                        "eta": { "type": "string" },
                        "notes": { "type": "string" }
                    },
                    "required": ["trip_id", "sequence"]
                }
            },
            {
                "name": "check_call",
                "description": "Record a driver check-call event with current location and optional notes and next-stop ETA.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" },
                        "location": { "type": "string" },
                        "notes": { "type": "string" },
                        "eta_next_stop": { "type": "string" }
                    },
                    "required": ["trip_id", "location"]
                }
            },
            {
                "name": "list_drivers",
                "description": "List drivers. Optional filter: status (available/assigned/dispatched/inactive).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "status": { "type": "string", "enum": ["available","assigned","dispatched","inactive"] }
                    }
                }
            },
            {
                "name": "get_driver",
                "description": "Get a single driver by UUID.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "format": "uuid" }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "list_trucks",
                "description": "List all trucks.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "get_truck",
                "description": "Get a single truck by UUID.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "truck_id": { "type": "string", "format": "uuid" } },
                    "required": ["truck_id"]
                }
            },
            {
                "name": "create_truck",
                "description": "Create a new truck. Defaults status to `available`. Unknown fields are rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "unit_number": { "type": "string" },
                        "year":        { "type": "integer" },
                        "make":        { "type": "string" },
                        "model":       { "type": "string" },
                        "vin":         { "type": "string" },
                        "plate":       { "type": "string" },
                        "plate_state": { "type": "string" },
                        "notes":       { "type": "string" }
                    },
                    "required": ["unit_number"]
                }
            },
            {
                "name": "update_truck",
                "description": "Update a truck's fields. `status` is not settable here — trucks transition via the trip lifecycle. Unknown fields are rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "truck_id":    { "type": "string", "format": "uuid" },
                        "unit_number": { "type": "string" },
                        "year":        { "type": "integer" },
                        "make":        { "type": "string" },
                        "model":       { "type": "string" },
                        "vin":         { "type": "string" },
                        "plate":       { "type": "string" },
                        "plate_state": { "type": "string" },
                        "notes":       { "type": "string" }
                    },
                    "required": ["truck_id"]
                }
            },
            {
                "name": "list_trailers",
                "description": "List all trailers.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "get_trailer",
                "description": "Get a single trailer by UUID.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "trailer_id": { "type": "string", "format": "uuid" } },
                    "required": ["trailer_id"]
                }
            },
            {
                "name": "create_trailer",
                "description": "Create a new trailer. `owner` is one of fleet/carrier/customer/other; `owner_name` is required when owner is not fleet. Defaults status to `available`. Unknown fields are rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "unit_number":  { "type": "string" },
                        "owner":        { "type": "string", "enum": ["fleet","carrier","customer","other"] },
                        "owner_name":   { "type": "string" },
                        "year":         { "type": "integer" },
                        "make":         { "type": "string" },
                        "trailer_type": { "type": "string" },
                        "length_ft":    { "type": "number" },
                        "vin":          { "type": "string" },
                        "plate":        { "type": "string" },
                        "plate_state":  { "type": "string" },
                        "notes":        { "type": "string" }
                    },
                    "required": ["unit_number", "owner"]
                }
            },
            {
                "name": "update_trailer",
                "description": "Update a trailer's fields. `status` is not settable here — trailers transition via the trip lifecycle. Unknown fields are rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trailer_id":   { "type": "string", "format": "uuid" },
                        "unit_number":  { "type": "string" },
                        "owner":        { "type": "string", "enum": ["fleet","carrier","customer","other"] },
                        "owner_name":   { "type": "string" },
                        "year":         { "type": "integer" },
                        "make":         { "type": "string" },
                        "trailer_type": { "type": "string" },
                        "length_ft":    { "type": "number" },
                        "vin":          { "type": "string" },
                        "plate":        { "type": "string" },
                        "plate_state":  { "type": "string" },
                        "notes":        { "type": "string" }
                    },
                    "required": ["trailer_id"]
                }
            },
            {
                "name": "list_events",
                "description": "List events. Optional filters: trip_id, driver_id.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" },
                        "driver_id": { "type": "string", "format": "uuid" }
                    }
                }
            },
            {
                "name": "list_facilities",
                "description": "List facilities. Optional q is a case-insensitive substring search across name and address. limit defaults to 100.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "q":     { "type": "string" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 1000 }
                    }
                }
            },
            {
                "name": "get_facility",
                "description": "Get a single facility by UUID.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "facility_id": { "type": "string", "format": "uuid" }
                    },
                    "required": ["facility_id"]
                }
            },
            {
                "name": "create_facility",
                "description": "Create a new facility. When lat+lng are omitted the geocoder is queued; when both are provided the facility is marked geocoded immediately. Unknown fields are rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name":      { "type": "string" },
                        "address":   { "type": "string" },
                        "contacts":  { "type": "array", "items": { "type": "object" } },
                        "notes":     { "type": "string" },
                        "tags":      { "type": "array", "items": { "type": "string" } },
                        "blob_ids":  { "type": "array", "items": { "type": "string", "format": "uuid" } },
                        "lat":       { "type": "number" },
                        "lng":       { "type": "number" }
                    },
                    "required": ["name", "address"]
                }
            },
            {
                "name": "update_facility",
                "description": "Update a facility's fields. Setting `address` re-queues the geocoder; explicit `lat`+`lng` skip the geocoder and mark the record geocoded. Unknown fields are rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "facility_id": { "type": "string", "format": "uuid" },
                        "name":        { "type": "string" },
                        "address":     { "type": "string" },
                        "contacts":    { "type": "array", "items": { "type": "object" } },
                        "notes":       { "type": "string" },
                        "tags":        { "type": "array", "items": { "type": "string" } },
                        "blob_ids":    { "type": "array", "items": { "type": "string", "format": "uuid" } },
                        "lat":         { "type": "number" },
                        "lng":         { "type": "number" }
                    },
                    "required": ["facility_id"]
                }
            },
            {
                "name": "trip_doctor",
                "description": "Diagnose a trip's data integrity. Returns a structured report of findings (missing stop metadata, broken chain links, stale mileage arithmetic, status/actuals mismatches, unresolved driver/truck/trailer ids). Dry-run by default. Pass apply=true to commit safe auto-fixes (currently: resync trip-stop fields from the linked load when the trip's fields are null and the load has values; never overwrites a non-null trip value).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" },
                        "apply":   { "type": "boolean", "default": false }
                    },
                    "required": ["trip_id"]
                }
            },
            {
                "name": "load_doctor",
                "description": "Diagnose a load's data integrity. Checks: stop facilities geocoded, scheduled windows well-formed, actual_depart > actual_arrive, timezone present when actuals are, rate_items sum matches total. Read-only — surfaces findings; fixes live in facility_doctor or require human reconciliation.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "load_id": { "type": "string", "format": "uuid" },
                        "apply":   { "type": "boolean", "default": false }
                    },
                    "required": ["load_id"]
                }
            },
            {
                "name": "facility_doctor",
                "description": "Diagnose a facility's data integrity. Checks: address present, lat/lng present, coordinates inside US bounding box (warning), normalized_address present when geocoded. With apply=true, re-queues geocoding for facilities stuck at geocode_status=permanently_failed (resets failure count, sets status=pending, pushes onto the geocoding worker). Setting manual coordinates remains a deliberate dispatcher action via update_facility.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "facility_id": { "type": "string", "format": "uuid" },
                        "apply":       { "type": "boolean", "default": false }
                    },
                    "required": ["facility_id"]
                }
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// Tool dispatch helpers
// ---------------------------------------------------------------------------

fn parse_uuid(args: &Value, key: &str) -> Result<Uuid, String> {
    let s = args[key].as_str().ok_or_else(|| format!("missing or non-string field '{key}'"))?;
    s.parse::<Uuid>().map_err(|_| format!("invalid UUID for field '{key}': {s}"))
}

fn parse_uuid_opt(args: &Value, key: &str) -> Result<Option<Uuid>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| format!("field '{key}' must be a string UUID"))?;
            Ok(Some(s.parse::<Uuid>().map_err(|_| format!("invalid UUID for field '{key}': {s}"))?))
        }
    }
}

// ---------------------------------------------------------------------------
// Main handler
// ---------------------------------------------------------------------------

pub async fn handle(
    State(state): State<AppState>,
    Json(req): Json<JsonRpcRequest>,
) -> Json<JsonRpcResponse> {
    let id = req.id.clone();

    if req.jsonrpc != "2.0" {
        return Json(JsonRpcResponse::err(id, -32600, "invalid JSON-RPC version"));
    }

    let result = match req.method.as_str() {
        "initialize" => handle_initialize(),
        "tools/list" => Ok(tools_list()),
        "tools/call" => handle_tool_call(&state, &req.params).await,
        _ => return Json(JsonRpcResponse::err(id, -32601, format!("method not found: {}", req.method))),
    };

    match result {
        Ok(value) => Json(JsonRpcResponse::ok(id, value)),
        Err(e) => Json(JsonRpcResponse::err(id, -32603, e)),
    }
}

fn handle_initialize() -> Result<Value, String> {
    Ok(json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "ollie-dispatcher", "version": "1.0" }
    }))
}

async fn handle_tool_call(state: &AppState, params: &Value) -> Result<Value, String> {
    let name = params["name"].as_str().ok_or("missing tool name")?;
    let args = &params["arguments"];

    match name {
        "list_loads" => tool_list_loads(state, args).await,
        "get_load" => tool_get_load(state, args).await,
        "create_load" => tool_create_load(state, args).await,
        "update_load" => tool_update_load(state, args).await,
        "list_trips" => tool_list_trips(state, args).await,
        "get_trip" => tool_get_trip(state, args).await,
        "create_trip" => tool_create_trip(state, args).await,
        "update_trip" => tool_update_trip(state, args).await,
        "recalculate_trip_miles" => tool_recalculate_trip_miles(state, args).await,
        "assign_driver" => tool_assign_driver(state, args).await,
        "unassign_driver" => tool_unassign_driver(state, args).await,
        "dispatch_trip" => tool_dispatch_trip(state, args).await,
        "undispatch_trip" => tool_undispatch_trip(state, args).await,
        "cancel_trip" => tool_cancel_trip(state, args).await,
        "complete_trip" => tool_complete_trip(state, args).await,
        "stop_arrive" => tool_stop_arrive(state, args).await,
        "stop_depart" => tool_stop_depart(state, args).await,
        "stop_late" => tool_stop_late(state, args).await,
        "check_call" => tool_check_call(state, args).await,
        "list_drivers" => tool_list_drivers(state, args).await,
        "get_driver" => tool_get_driver(state, args).await,
        "list_trucks" => tool_list_trucks(state).await,
        "get_truck" => tool_get_truck(state, args).await,
        "create_truck" => tool_create_truck(state, args).await,
        "update_truck" => tool_update_truck(state, args).await,
        "list_trailers" => tool_list_trailers(state).await,
        "get_trailer" => tool_get_trailer(state, args).await,
        "create_trailer" => tool_create_trailer(state, args).await,
        "update_trailer" => tool_update_trailer(state, args).await,
        "list_events" => tool_list_events(state, args).await,
        "list_facilities" => tool_list_facilities(state, args).await,
        "get_facility" => tool_get_facility(state, args).await,
        "create_facility" => tool_create_facility(state, args).await,
        "update_facility" => tool_update_facility(state, args).await,
        "trip_doctor" => tool_trip_doctor(state, args).await,
        "load_doctor" => tool_load_doctor(state, args).await,
        "facility_doctor" => tool_facility_doctor(state, args).await,
        _ => Err(format!("unknown tool: {name}")),
    }
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

async fn tool_list_loads(state: &AppState, args: &Value) -> Result<Value, String> {
    let status = args["status"].as_str();
    let limit = 100usize;
    let offset = 0usize;

    let (total, items) = state.db.list_loads(
        status,
        None, // customer
        &[],  // tags
        None, // from
        None, // to
        limit,
        offset,
    ).await.map_err(|e| e.to_string())?;

    Ok(mcp_content(serde_json::json!({ "returned": total, "items": items })))
}

async fn tool_get_load(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let record = state.db.get_load_by_id(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_create_load(state: &AppState, args: &Value) -> Result<Value, String> {
    use crate::models::CreateLoadRequest;
    use crate::api::loads::resolve_stops_pub;

    let req: CreateLoadRequest = serde_json::from_value(args.clone())
        .map_err(|e| format!("invalid create_load arguments: {e}"))?;

    let stops = resolve_stops_pub(state, req.stops).await.map_err(|e| e.to_string())?;

    let now = chrono::Utc::now();
    let load_number = match req.load_number {
        Some(n) => n,
        None => {
            use chrono::Datelike;
            state.db.next_load_number(now.year()).await.map_err(|e| e.to_string())?
        }
    };

    let facility_ids: Vec<Uuid> = stops.iter().map(|s| s.facility_id).collect();
    let facilities = state.db.batch_get_facilities(&facility_ids).await.map_err(|e| e.to_string())?;
    let stop_text = stops.iter()
        .filter_map(|s| facilities.get(&s.facility_id))
        .map(|f| format!("{} {}", f.name, f.address))
        .collect::<Vec<_>>()
        .join(" ");
    let embed_text = format!(
        "{} {} {} {} {}",
        req.customer_name,
        stop_text,
        req.commodity.as_deref().unwrap_or(""),
        req.notes.as_deref().unwrap_or(""),
        req.tags.join(" "),
    );
    let embedding = crate::ai::embed::embed_text(&state.ai, &embed_text).await.ok();

    let record = crate::models::LoadRecord {
        id: Uuid::new_v4(),
        load_number,
        owner_id: 0,
        status: LoadStatus::Planned,
        customer_name: req.customer_name,
        customer_ref: req.customer_ref,
        stops,
        rate_items: req.rate_items,
        commodity: req.commodity,
        weight_lbs: req.weight_lbs,
        miles: req.miles,
        notes: req.notes,
        tags: req.tags,
        blob_ids: req.blob_ids,
        invoice_number: None,
        invoice_date: None,
        cancellation_reason: None,
        embedding,
        created_at: now,
        updated_at: now,
    };

    state.db.insert_load(&record).await.map_err(|e| e.to_string())?;

    if record.miles.is_none() {
        let _ = state.routing_tx.try_send(record.id);
    }

    Ok(mcp_content(record))
}

async fn tool_update_load(state: &AppState, args: &Value) -> Result<Value, String> {
    use crate::models::UpdateLoadRequest;
    use crate::api::loads::resolve_stops_pub;

    let id = parse_uuid(args, "id")?;

    let req: UpdateLoadRequest = serde_json::from_value(args.clone())
        .map_err(|e| format!("invalid update_load arguments: {e}"))?;

    let stops_provided = req.stops.is_some();
    let stops = match req.stops {
        Some(inputs) => Some(resolve_stops_pub(state, inputs).await.map_err(|e| e.to_string())?),
        None => None,
    };

    let existing = state.db.get_load_by_id(id).await.map_err(|e| e.to_string())?;
    let effective_stops = stops.as_ref().unwrap_or(&existing.stops);
    let facility_ids: Vec<Uuid> = effective_stops.iter().map(|s| s.facility_id).collect();
    let facilities = state.db.batch_get_facilities(&facility_ids).await.map_err(|e| e.to_string())?;
    let stop_text = effective_stops.iter()
        .filter_map(|s| facilities.get(&s.facility_id))
        .map(|f| format!("{} {}", f.name, f.address))
        .collect::<Vec<_>>()
        .join(" ");
    let embed_text = format!(
        "{} {} {} {} {}",
        req.customer_name.as_deref().unwrap_or(&existing.customer_name),
        stop_text,
        req.commodity.as_deref().unwrap_or(existing.commodity.as_deref().unwrap_or("")),
        req.notes.as_deref().unwrap_or(existing.notes.as_deref().unwrap_or("")),
        req.tags.as_ref().unwrap_or(&existing.tags).join(" "),
    );
    let embedding = crate::ai::embed::embed_text(&state.ai, &embed_text).await.ok();

    let mut updated = state.db.update_load_metadata(
        id,
        req.customer_name,
        req.customer_ref,
        stops,
        req.rate_items,
        req.commodity,
        req.weight_lbs,
        req.miles,
        req.notes,
        req.tags,
        req.blob_ids,
        embedding,
    ).await.map_err(|e| e.to_string())?;

    if stops_provided && req.miles.is_none() {
        state.db.clear_load_miles(id).await.map_err(|e| e.to_string())?;
        updated.miles = None;
        let _ = state.routing_tx.try_send(id);
    }

    Ok(mcp_content(updated))
}

async fn tool_list_trips(state: &AppState, args: &Value) -> Result<Value, String> {
    let q = super::data::ListTripsQuery {
        load_id: parse_uuid_opt(args, "load_id")?,
        driver_id: parse_uuid_opt(args, "driver_id")?,
        status: args["status"].as_str().map(|s| s.to_string()),
        trip_number: args["trip_number"].as_str().map(|s| s.to_string()),
        load_number: args["load_number"].as_str().map(|s| s.to_string()),
    };
    let items = super::data::build_trip_list_items(state, q).await
        .map_err(|e| e.to_string())?;
    let returned = items.len();
    Ok(mcp_content(serde_json::json!({ "returned": returned, "items": items })))
}

async fn tool_get_trip(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let item = super::data::build_trip_detail(state, id).await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(item))
}

async fn tool_create_trip(state: &AppState, args: &Value) -> Result<Value, String> {
    use crate::models::trip::CreateTripRequest;
    let req: CreateTripRequest = serde_json::from_value(args.clone())
        .map_err(|e| format!("invalid create_trip arguments: {e}"))?;

    // Reuse the admin create_trip handler — pure DB work, no HTTP roundtrip.
    let _resp = crate::api::trips::create_trip(
        axum::extract::State(state.clone()),
        Json(req),
    )
    .await
    .map_err(|e| e.to_string())?;

    // Re-fetch most recently created trip via the dispatcher-enriched detail
    // builder so the MCP response carries a full mileage_summary. We need the
    // id — the admin handler returns it via the IntoResponse, but rather than
    // dig into axum Response internals, look up by sorting trips by created_at.
    // Simpler: scan once; production scale is fine for MCP audits.
    let all = state.db.list_trips(None, None, None).await
        .map_err(|e| e.to_string())?;
    let newest = all.iter().max_by_key(|t| t.created_at)
        .ok_or("trip create succeeded but trip not found on re-fetch")?;
    let detail = super::data::build_trip_detail(state, newest.id).await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(detail))
}

async fn tool_update_trip(state: &AppState, args: &Value) -> Result<Value, String> {
    use super::trip_writes::{apply_trip_patch, PatchTripBody};
    let trip_id = parse_uuid(args, "trip_id")?;

    let mut body = args.clone();
    if let Value::Object(map) = &mut body {
        map.remove("trip_id");
    }

    // Validate shape early so the agent gets a clear error before we touch DB.
    let _check: PatchTripBody = serde_json::from_value(body.clone())
        .map_err(|e| format!("invalid update_trip arguments: {e}"))?;

    let result = apply_trip_patch(state, trip_id, body).await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(result))
}

async fn tool_recalculate_trip_miles(state: &AppState, args: &Value) -> Result<Value, String> {
    use super::trip_writes::{recalculate_miles_handler, RecalculateMilesBody};
    let trip_id = parse_uuid(args, "trip_id")?;
    let force = args["force"].as_bool().unwrap_or(false);

    let body = Some(Json(RecalculateMilesBody { force }));
    let _resp = recalculate_miles_handler(
        axum::extract::State(state.clone()),
        Path(trip_id),
        body,
    )
    .await
    .map_err(|e| e.to_string())?;

    let trip = state.db.get_trip(trip_id).await.map_err(|e| e.to_string())?;
    let summary = crate::api::mileage_summary::build_mileage_summary(state, &trip).await;
    Ok(mcp_content(summary))
}

async fn tool_assign_driver(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;
    let driver_id = parse_uuid(args, "driver_id")?;
    let truck_id = parse_uuid(args, "truck_id")?;
    let trailer_ids: Vec<Uuid> = match args.get("trailer_ids") {
        None | Some(Value::Null) => vec![],
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid trailer_ids: {e}"))?,
    };

    let driver = state.db.get_driver_by_id(driver_id).await.map_err(|e| e.to_string())?;
    if driver.status != DriverStatus::Available {
        return Err(format!("driver {driver_id} is not available"));
    }

    let truck = state.db.get_truck_by_id(truck_id).await.map_err(|e| e.to_string())?;
    if truck.status != TruckStatus::Available {
        return Err(format!("truck {truck_id} is not available"));
    }

    for &tid in &trailer_ids {
        let trailer = state.db.get_trailer_by_id(tid).await.map_err(|e| e.to_string())?;
        if trailer.status != TrailerStatus::Available {
            return Err(format!("trailer {tid} is not available"));
        }
    }

    state.db.transition_trip_status(trip_id, TripStatus::Assigned).await.map_err(|e| e.to_string())?;
    state.db.update_trip_resources(trip_id, Some(driver_id), Some(truck_id), trailer_ids.clone())
        .await.map_err(|e| e.to_string())?;

    state.db.update_driver_status(driver_id, DriverStatus::Assigned).await.map_err(|e| e.to_string())?;
    state.db.update_truck_status(truck_id, TruckStatus::Assigned).await.map_err(|e| e.to_string())?;
    for &tid in &trailer_ids {
        state.db.update_trailer_status(tid, TrailerStatus::Assigned).await.map_err(|e| e.to_string())?;
    }

    // Re-fetch after all mutations (stale-return rule)
    let trip = state.db.get_trip(trip_id).await.map_err(|e| e.to_string())?;

    if let Some(load_id) = trip.load_id {
        if let Ok(load) = state.db.get_load_by_id(load_id).await {
            if load.status == LoadStatus::Planned {
                let _ = state.db.transition_load_status(load_id, LoadStatus::Assigned, None, None, None).await;
            }
        }
    }

    events::on_trip_assigned(&state.db, trip_id).await;
    Ok(mcp_content(trip))
}

async fn tool_unassign_driver(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;

    let existing = state.db.get_trip(trip_id).await.map_err(|e| e.to_string())?;
    state.db.transition_trip_status(trip_id, TripStatus::Planned).await.map_err(|e| e.to_string())?;
    state.db.update_trip_resources(trip_id, None, None, vec![]).await.map_err(|e| e.to_string())?;

    if let Some(driver_id) = existing.driver_id {
        let _ = state.db.update_driver_status(driver_id, DriverStatus::Available).await;
    }
    if let Some(truck_id) = existing.truck_id {
        let _ = state.db.update_truck_status(truck_id, TruckStatus::Available).await;
    }
    for &tid in &existing.trailer_ids {
        let _ = state.db.update_trailer_status(tid, TrailerStatus::Available).await;
    }

    if let Some(load_id) = existing.load_id {
        let active = state.db.count_active_trips_for_load(load_id).await.unwrap_or(1);
        if active == 0 {
            if let Ok(load) = state.db.get_load_by_id(load_id).await {
                if load.status == LoadStatus::Assigned {
                    let _ = state.db.transition_load_status(load_id, LoadStatus::Planned, None, None, None).await;
                }
            }
        }
    }

    // Re-fetch after all mutations (stale-return rule)
    let trip = state.db.get_trip(trip_id).await.map_err(|e| e.to_string())?;
    events::on_trip_unassigned(&state.db, trip_id).await;
    Ok(mcp_content(trip))
}

// ---------------------------------------------------------------------------
// Trip lifecycle MCP tools — thin shims that invoke the admin trip_actions
// handler and return the resulting trip record (or status acknowledgement
// for 204 actions).
// ---------------------------------------------------------------------------

async fn tool_dispatch_trip(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;
    trip_actions::dispatch_trip(State(state.clone()), Path(trip_id))
        .await
        .map_err(|e| e.to_string())?;
    let trip = state.db.get_trip(trip_id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(trip))
}

async fn tool_undispatch_trip(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;
    trip_actions::undispatch_trip(State(state.clone()), Path(trip_id))
        .await
        .map_err(|e| e.to_string())?;
    let trip = state.db.get_trip(trip_id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(trip))
}

async fn tool_cancel_trip(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;
    trip_actions::cancel_trip(State(state.clone()), Path(trip_id))
        .await
        .map_err(|e| e.to_string())?;
    let trip = state.db.get_trip(trip_id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(trip))
}

async fn tool_complete_trip(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;
    trip_actions::complete_trip(State(state.clone()), Path(trip_id))
        .await
        .map_err(|e| e.to_string())?;
    let trip = state.db.get_trip(trip_id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(trip))
}

async fn tool_stop_arrive(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;
    let sequence = args["sequence"]
        .as_u64()
        .ok_or("missing or non-integer field 'sequence'")? as u32;
    let actual_arrive = args["actual_arrive"]
        .as_str()
        .ok_or("missing or non-string field 'actual_arrive'")?
        .to_string();
    trip_actions::stop_arrive(
        State(state.clone()),
        Path((trip_id, sequence)),
        Json(StopArriveRequest { actual_arrive }),
    )
    .await
    .map_err(|e| e.to_string())?;
    let trip = state.db.get_trip(trip_id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(trip))
}

async fn tool_stop_depart(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;
    let sequence = args["sequence"]
        .as_u64()
        .ok_or("missing or non-integer field 'sequence'")? as u32;
    let actual_depart = args["actual_depart"]
        .as_str()
        .ok_or("missing or non-string field 'actual_depart'")?
        .to_string();
    trip_actions::stop_depart(
        State(state.clone()),
        Path((trip_id, sequence)),
        Json(StopDepartRequest { actual_depart }),
    )
    .await
    .map_err(|e| e.to_string())?;
    let trip = state.db.get_trip(trip_id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(trip))
}

async fn tool_stop_late(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;
    let sequence = args["sequence"]
        .as_u64()
        .ok_or("missing or non-integer field 'sequence'")? as u32;
    let eta = args["eta"].as_str().map(|s| s.to_string());
    let notes = args["notes"].as_str().map(|s| s.to_string());
    trip_actions::stop_late(
        State(state.clone()),
        Path((trip_id, sequence)),
        Json(StopLateRequest { eta, notes }),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "trip_id": trip_id, "sequence": sequence, "status": "late_flag_recorded" })))
}

async fn tool_check_call(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;
    let location = args["location"]
        .as_str()
        .ok_or("missing or non-string field 'location'")?
        .to_string();
    let notes = args["notes"].as_str().map(|s| s.to_string());
    let eta_next_stop = args["eta_next_stop"].as_str().map(|s| s.to_string());
    trip_actions::check_call(
        State(state.clone()),
        Path(trip_id),
        Json(CheckCallRequest { location, notes, eta_next_stop }),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "trip_id": trip_id, "status": "check_call_recorded" })))
}

async fn tool_list_drivers(state: &AppState, args: &Value) -> Result<Value, String> {
    let status = args["status"].as_str();
    let (total, items) = state.db.list_drivers(status, 100, 0)
        .await.map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "returned": total, "items": items })))
}

async fn tool_get_driver(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let record = state.db.get_driver_by_id(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_list_trucks(state: &AppState) -> Result<Value, String> {
    let (total, items) = state.db.list_trucks(None, 100, 0)
        .await.map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "returned": total, "items": items })))
}

async fn tool_list_trailers(state: &AppState) -> Result<Value, String> {
    let (total, items) = state.db.list_trailers(None, None, 100, 0)
        .await.map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "returned": total, "items": items })))
}

async fn tool_get_truck(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "truck_id")?;
    let record = state.db.get_truck_by_id(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_create_truck(state: &AppState, args: &Value) -> Result<Value, String> {
    let record = super::truck_writes::apply_truck_create(state, args.clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_update_truck(state: &AppState, args: &Value) -> Result<Value, String> {
    let truck_id = parse_uuid(args, "truck_id")?;
    let mut body = args.clone();
    if let Value::Object(map) = &mut body {
        map.remove("truck_id");
    }
    let record = super::truck_writes::apply_truck_patch(state, truck_id, body)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_get_trailer(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "trailer_id")?;
    let record = state.db.get_trailer_by_id(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_create_trailer(state: &AppState, args: &Value) -> Result<Value, String> {
    let record = super::trailer_writes::apply_trailer_create(state, args.clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_update_trailer(state: &AppState, args: &Value) -> Result<Value, String> {
    let trailer_id = parse_uuid(args, "trailer_id")?;
    let mut body = args.clone();
    if let Value::Object(map) = &mut body {
        map.remove("trailer_id");
    }
    let record = super::trailer_writes::apply_trailer_patch(state, trailer_id, body)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_list_events(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid_opt(args, "trip_id")?;
    let driver_id = parse_uuid_opt(args, "driver_id")?;
    // trip_id takes priority as entity_id filter
    let entity_id = trip_id.or(driver_id);

    let (_total, records) = state.db.query_events(
        entity_id,
        None,
        None,
        None,
        None,
        20,
        0,
    ).await.map_err(|e| e.to_string())?;

    let items: Vec<crate::models::EventResponse> = records.into_iter().map(crate::models::EventResponse::from).collect();
    Ok(mcp_content(serde_json::json!({ "returned": items.len(), "items": items })))
}

// ---------------------------------------------------------------------------
// Facilities — list / get / create / update share the dispatcher write helpers
// in `facility_writes` so HTTP and MCP enforce the same validation + side
// effects (geocode queue, manual-coords override).
// ---------------------------------------------------------------------------

async fn tool_list_facilities(state: &AppState, args: &Value) -> Result<Value, String> {
    let limit = args["limit"].as_u64().map(|n| n as usize).unwrap_or(100).min(1000);
    let q = args["q"].as_str().map(|s| s.to_string());

    let (_total, items) = state.db.list_facilities(None, &[], 1000, 0)
        .await.map_err(|e| e.to_string())?;

    let filtered: Vec<_> = if let Some(needle) = q.as_deref().filter(|s| !s.is_empty()) {
        let needle = needle.to_lowercase();
        items.into_iter()
            .filter(|f| {
                f.name.to_lowercase().contains(&needle)
                    || f.address.to_lowercase().contains(&needle)
            })
            .take(limit)
            .collect()
    } else {
        items.into_iter().take(limit).collect()
    };
    let returned = filtered.len();
    Ok(mcp_content(serde_json::json!({ "returned": returned, "items": filtered })))
}

async fn tool_get_facility(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "facility_id")?;
    let record = state.db.get_facility_by_id(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_create_facility(state: &AppState, args: &Value) -> Result<Value, String> {
    let record = super::facility_writes::apply_facility_create(state, args.clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_update_facility(state: &AppState, args: &Value) -> Result<Value, String> {
    let facility_id = parse_uuid(args, "facility_id")?;
    let mut body = args.clone();
    if let Value::Object(map) = &mut body {
        map.remove("facility_id");
    }
    let record = super::facility_writes::apply_facility_patch(state, facility_id, body)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

// ---------------------------------------------------------------------------
// Doctors (dry-run + diff-and-confirm). See `services::doctors` for the
// check definitions; these wrappers are pure transport glue.
// ---------------------------------------------------------------------------

async fn tool_trip_doctor(state: &AppState, args: &Value) -> Result<Value, String> {
    let trip_id = parse_uuid(args, "trip_id")?;
    let apply = args["apply"].as_bool().unwrap_or(false);
    let report = crate::services::doctors::trip::run(state, trip_id, apply)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(report))
}

async fn tool_load_doctor(state: &AppState, args: &Value) -> Result<Value, String> {
    let load_id = parse_uuid(args, "load_id")?;
    let apply = args["apply"].as_bool().unwrap_or(false);
    let report = crate::services::doctors::load::run(state, load_id, apply)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(report))
}

async fn tool_facility_doctor(state: &AppState, args: &Value) -> Result<Value, String> {
    let facility_id = parse_uuid(args, "facility_id")?;
    let apply = args["apply"].as_bool().unwrap_or(false);
    let report = crate::services::doctors::facility::run(state, facility_id, apply)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(report))
}
