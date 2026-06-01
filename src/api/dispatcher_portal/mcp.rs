// src/api/dispatcher_portal/mcp.rs
//
// MCP server for the dispatcher agent surface, built on rmcp's Streamable HTTP
// transport (the official Rust MCP SDK, adopted in #105 once the project moved
// to Axum 0.8). rmcp owns the JSON-RPC envelope, protocol-version negotiation,
// the MCP-Protocol-Version header check, the notifications→202 behaviour, and
// session/SSE plumbing. This module only implements the semantic ServerHandler
// (server info + tool list + tool dispatch) and wires the 47 existing tool
// shims into it; no business logic lives here.
//
// Auth is enforced at the HTTP layer — the require_dispatcher_auth route_layer
// in mod.rs runs BEFORE the request reaches this service, so rmcp tool handlers
// receive no auth context and enforce none themselves.
//
// Transport config (see `mcp_service`): stateful Streamable HTTP. An `initialize`
// POST opens a session (returned in the Mcp-Session-Id header) and responses
// stream back as text/event-stream — the server→client channel that resource
// subscriptions (#299) and elicitation (#300) build on.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use base64::Engine;
use rmcp::{
    handler::server::ServerHandler,
    model::{
        AnnotateAble, CallToolRequestParams, CallToolResult, CompleteRequestParams, CompleteResult,
        CompletionInfo, Content, Implementation, InitializeResult, ListResourceTemplatesResult,
        ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion, RawResource,
        RawResourceTemplate, ReadResourceRequestParams, ReadResourceResult, ResourceContents,
        ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ErrorData as McpError, RoleServer,
};
use serde::Serialize;
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

use super::blob_links::{self, BlobUrlOp};

// ---------------------------------------------------------------------------
// rmcp ServerHandler
// ---------------------------------------------------------------------------

/// The dispatcher MCP server. Holds shared app state; the transport's service
/// factory builds one per session (a cheap clone of `AppState`).
#[derive(Clone)]
pub struct OllieMcp {
    state: AppState,
}

impl OllieMcp {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl ServerHandler for OllieMcp {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_completions()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2025_06_18)
        .with_server_info(Implementation::new(
            "ollie-dispatcher",
            env!("CARGO_PKG_VERSION"),
        ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tool_catalog()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // Scope enforcement (#331): the auth middleware inserted the authenticated
        // dispatcher's DispatcherClaims into the HTTP request extensions; rmcp
        // forwards the request Parts into the RequestContext extensions. Resolve the
        // effective scopes from there and gate the tool before any side effect.
        let scopes = dispatcher_scopes(&context);
        if let Some(required) = tool_required_scope(&request.name) {
            if !crate::models::permission::scope_granted(&scopes, required) {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "{} denied: missing required scope '{required}'.",
                    request.name
                ))]));
            }
        }
        // The authenticated caller's dispatcher id (for owner-protection/transfer
        // checks in the Users tools). Absent for an API-key principal with no
        // parseable id — those tools then treat the caller as least-privileged.
        let caller_id = dispatcher_caller_id(&context);
        let args = Value::Object(request.arguments.unwrap_or_default());
        // Destructive ops ask the user to confirm via elicitation when the client
        // supports it; clients that don't degrade to the prior behavior (#300).
        if let Some(declined) = confirm_destructive(&request.name, &args, &context.peer).await {
            return Ok(declined);
        }
        match handle_tool_call(&self.state, &request.name, &args, &scopes, caller_id).await {
            // Emit the payload as structuredContent (typed, schema-checkable) AND a
            // backward-compatible JSON text block, per MCP 2025-06-18 (#293). Blob-
            // returning tools also attach resource_link items pointing at the
            // ollie://blob/{id} resources (#294).
            Ok(value) => {
                let links = blob_resource_links(&request.name, &value, &args);
                let mut result = CallToolResult::structured(value);
                result.content.extend(links);
                Ok(result)
            }
            // Domain failures ("trip can't be cancelled, it's in_transit") are
            // recoverable feedback the model should read and adapt to, so they come
            // back as a normal result with isError: true — NOT a JSON-RPC error.
            Err(ToolError::Domain(msg)) => Ok(CallToolResult::error(vec![Content::text(msg)])),
            // An unknown tool name is a genuine protocol fault → JSON-RPC error.
            Err(ToolError::Unknown) => Err(McpError::invalid_params(
                format!("unknown tool: {}", request.name),
                None,
            )),
        }
    }

    /// Browse blobs as first-class MCP resources (paginated). Each blob is exposed
    /// at `ollie://blob/{id}`; load/trip resources are reachable by templated URI
    /// (see `list_resource_templates`) but aren't enumerated here.
    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let offset = resource_offset(request.and_then(|r| r.cursor).as_deref())?;
        let (total, blobs) = self
            .state
            .db
            .list(None, &[], PAGE_SIZE, offset)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let returned = blobs.len();
        let resources = blobs.iter().map(blob_resource).collect();
        let mut result = ListResourcesResult::with_all_items(resources);
        if offset + returned < total {
            result.next_cursor = Some(encode_offset(offset + returned));
        }
        Ok(result)
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult::with_all_items(resource_templates()))
    }

    /// Resolve an `ollie://{kind}/{id}` URI to a JSON view of the record.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let contents = read_ollie_resource(&self.state, &request.uri).await?;
        Ok(ReadResourceResult::new(vec![contents]))
    }

    /// Autocomplete high-cardinality reference arguments (customer_name, facility
    /// `q`, tag) from existing records, so an interactive client/agent can discover
    /// valid values without a separate list round-trip (#301).
    async fn complete(
        &self,
        request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, McpError> {
        let values = completion_values(&self.state, &request).await?;
        let info =
            CompletionInfo::with_all_values(values).map_err(|e| McpError::internal_error(e, None))?;
        Ok(CompleteResult::new(info))
    }
}

/// Why a `tools/call` did not produce a result. `Domain` failures are recoverable
/// tool-execution feedback (surfaced as an isError result); `Unknown` is a
/// protocol fault (surfaced as a JSON-RPC error).
enum ToolError {
    Unknown,
    Domain(String),
}

// ---------------------------------------------------------------------------
// Scope enforcement (#331)
//
// HTTP auth (require_dispatcher_auth) runs before rmcp and inserts the
// authenticated DispatcherClaims (carrying server-computed effective_scopes)
// into the axum request extensions. rmcp forwards the remaining
// `http::request::Parts` into RequestContext.extensions, so the tool layer
// recovers the caller's scopes from there and gates each tool by the same
// resource:action rules the HTTP routes use.
// ---------------------------------------------------------------------------

/// Recover the caller's effective scopes from the forwarded HTTP request parts.
/// Empty when the claims are absent — which denies every gated tool (fail-closed).
fn dispatcher_scopes(context: &RequestContext<RoleServer>) -> Vec<String> {
    context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<super::jwt::DispatcherClaims>())
        .map(|claims| claims.effective_scopes.clone())
        .unwrap_or_default()
}

/// Recover the authenticated caller's dispatcher id from the forwarded HTTP
/// request parts, for owner-protection/transfer checks in the Users tools.
/// `None` when claims are absent or the id is not a UUID (e.g. an API-key
/// principal); the Users tools then treat the caller as least-privileged.
fn dispatcher_caller_id(context: &RequestContext<RoleServer>) -> Option<Uuid> {
    context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<super::jwt::DispatcherClaims>())
        .and_then(|claims| claims.dispatcher_id.parse::<Uuid>().ok())
}

/// Build a minimal DispatcherClaims carrying only the given effective scopes, for
/// passing the caller's authority into an HTTP handler reused as an MCP shim (e.g.
/// `recalculate_miles_handler`). Identity fields are placeholders — the handler
/// only consults `effective_scopes` via `require_scope`.
fn claims_with_scopes(scopes: &[String]) -> super::jwt::DispatcherClaims {
    super::jwt::DispatcherClaims {
        dispatcher_id: String::new(),
        token_version: 0,
        iss: String::new(),
        aud: String::new(),
        exp: 0,
        iat: 0,
        kid: String::new(),
        api_key_id: None,
        api_key_label: None,
        effective_scopes: scopes.to_vec(),
    }
}

/// The `resource:action` scope each tool requires. Mirrors the HTTP route map:
/// list_*/get_*/*_doctor/search_blobs → read, create_*/update_* → write,
/// delete_* → delete, with the elevated load verbs and equipment/pin/miles
/// special cases called out. Returns None only for tools that need no scope
/// (there are none today; every tool maps to one).
fn tool_required_scope(name: &str) -> Option<&'static str> {
    let scope = match name {
        // Loads
        "list_loads" | "get_load" => "loads:read",
        "create_load" | "update_load" => "loads:write",
        "delete_load" => "loads:delete",
        "settle_load" => "loads:settle",
        "invoice_load" => "loads:invoice",
        "cancel_load" => "loads:write",
        // Trips
        "list_trips" | "get_trip" => "trips:read",
        "create_trip" | "update_trip" | "assign_driver" | "unassign_driver"
        | "dispatch_trip" | "undispatch_trip" | "cancel_trip" | "complete_trip"
        | "stop_arrive" | "stop_depart" | "stop_late" | "check_call"
        | "recalculate_trip_miles" => "trips:write",
        "delete_trip" => "trips:delete",
        "trip_doctor" => "trips:read",
        // Drivers
        "list_drivers" | "get_driver" => "drivers:read",
        "create_driver" | "update_driver" | "attach_equipment" | "detach_equipment"
        | "set_driver_pin" => "drivers:write",
        "delete_driver" => "drivers:delete",
        // Trucks
        "list_trucks" | "get_truck" => "trucks:read",
        "create_truck" | "update_truck" => "trucks:write",
        "delete_truck" => "trucks:delete",
        // Trailers
        "list_trailers" | "get_trailer" => "trailers:read",
        "create_trailer" | "update_trailer" => "trailers:write",
        "delete_trailer" => "trailers:delete",
        // Facilities
        "list_facilities" | "get_facility" | "facility_doctor" => "facilities:read",
        "create_facility" | "update_facility" => "facilities:write",
        "delete_facility" => "facilities:delete",
        // Events
        "list_events" => "events:read",
        // Blobs
        "list_blobs" | "search_blobs" | "get_blob_url" | "get_blob_metadata" => "blobs:read",
        "upload_blob" => "blobs:write",
        "delete_blob" => "blobs:delete",
        // load_doctor reads a load's integrity.
        "load_doctor" => "loads:read",
        // Users (#331)
        "list_users" | "get_user" => "users:read",
        "create_user" | "update_user" | "reset_user_password" => "users:write",
        "delete_user" => "users:delete",
        _ => return None,
    };
    Some(scope)
}

// ---------------------------------------------------------------------------
// Elicitation (MCP `elicitation`, #300)
//
// Before a destructive op (cancel_trip, delete_blob force=true) the server asks
// the user to confirm — but only when the client declared elicitation support.
// Clients without it degrade to the prior behavior (the op runs as before), so
// existing integrations don't break. This is the one wired flow; disambiguating
// equipment selection etc. are tracked as follow-ups.
// ---------------------------------------------------------------------------

/// Structured response the client returns for a destructive-action confirmation.
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct DestructiveConfirmation {
    /// Set true to proceed with the destructive action.
    confirm: bool,
}
rmcp::elicit_safe!(DestructiveConfirmation);

/// Which tool calls warrant a destructive-action confirmation: cancelling a trip,
/// and force-deleting a blob (a non-force delete already errors on attachments).
fn is_destructive_op(name: &str, args: &Value) -> bool {
    match name {
        "cancel_trip" | "cancel_load" | "delete_load" | "delete_trip" | "delete_driver"
        | "delete_truck" | "delete_trailer" | "delete_facility" | "delete_user" => true,
        "delete_blob" => args["force"].as_bool() == Some(true),
        _ => false,
    }
}

/// Returns `Some(isError result)` if a destructive op must NOT proceed (the user
/// declined, cancelled, or confirmation was unavailable from a supporting client);
/// `None` to proceed (confirmed, or the client doesn't support elicitation).
async fn confirm_destructive(
    name: &str,
    args: &Value,
    peer: &rmcp::service::Peer<RoleServer>,
) -> Option<CallToolResult> {
    if !is_destructive_op(name, args) {
        return None;
    }
    // Graceful fallback: a client that didn't declare elicitation gets the prior
    // behavior (proceed without a round-trip).
    if peer.supported_elicitation_modes().is_empty() {
        return None;
    }
    let message = format!(
        "Confirm {name} ({})? This permanently changes fleet data and cannot be undone.",
        destructive_op_description(name)
    );
    // rmcp owns the elicit transport/deserialization; map its outcome to a simple
    // "did the user confirm?" and let `destructive_decision` (unit-tested) decide.
    let confirmed = match peer.elicit::<DestructiveConfirmation>(message).await {
        Ok(Some(c)) => Some(c.confirm),
        // No content / declined / cancelled / transport error.
        _ => None,
    };
    destructive_decision(name, confirmed)
}

/// Short human description of a destructive op, used in the confirmation prompt.
fn destructive_op_description(name: &str) -> &'static str {
    match name {
        "cancel_trip" => "cancel the trip",
        "cancel_load" => "cancel the load",
        "delete_load" => "delete the load record",
        "delete_trip" => "delete the trip record",
        "delete_driver" => "deactivate the driver and revoke their access",
        "delete_truck" => "deactivate the truck",
        "delete_trailer" => "deactivate the trailer",
        "delete_facility" => "delete the facility record",
        "delete_user" => "deactivate the user and revoke their access",
        "delete_blob" => "delete the blob",
        _ => "perform a destructive action",
    }
}

/// Decide whether a destructive op may proceed given the confirmation outcome:
/// `Some(true)` = explicitly confirmed → proceed (`None`); anything else (declined,
/// no content, error) → abort with an isError result.
fn destructive_decision(name: &str, confirmed: Option<bool>) -> Option<CallToolResult> {
    if confirmed == Some(true) {
        return None;
    }
    Some(CallToolResult::error(vec![Content::text(format!(
        "{name} was not performed: destructive-action confirmation was declined or unavailable."
    ))]))
}

// ---------------------------------------------------------------------------
// Resources (MCP `resources` capability, #299)
//
// Blobs are enumerable via resources/list under `ollie://blob/{id}`; loads and
// trips are reachable by templated URI (resources/read of `ollie://load/{id}` /
// `ollie://trip/{id}`) but not enumerated. read_resource returns a JSON view of
// the record. Cursor pagination mirrors the list-tool scheme.
// ---------------------------------------------------------------------------

/// Stable URI templates for the record types exposed as resources.
fn resource_templates() -> Vec<rmcp::model::ResourceTemplate> {
    [
        ("ollie://blob/{id}", "Blob", "A stored document/blob, by UUID."),
        ("ollie://load/{id}", "Load", "A freight load record, by UUID."),
        ("ollie://trip/{id}", "Trip", "A trip record, by UUID."),
    ]
    .into_iter()
    .map(|(uri, name, desc)| {
        RawResourceTemplate::new(uri, name)
            .with_description(desc)
            .with_mime_type("application/json")
            .no_annotation()
    })
    .collect()
}

/// Describe a blob as an MCP resource.
fn blob_resource(b: &crate::models::BlobListItem) -> rmcp::model::Resource {
    RawResource::new(format!("ollie://blob/{}", b.id), b.name.clone())
        .with_mime_type(b.mime_type.clone())
        .with_size(b.size.max(0) as u32)
        .no_annotation()
}

/// Build `resource_link` content items for blob-returning tools (#294), so clients
/// recognize blobs as referenceable `ollie://blob/{id}` resources (resolvable via
/// the Resources capability, #299) rather than opaque strings to parse. Fields
/// beyond the URI are attached where known. `upload_blob` returns a presigned POST
/// URL for a blob that does not exist yet, so it has nothing to link.
fn blob_resource_links(tool: &str, value: &Value, args: &Value) -> Vec<Content> {
    fn link_from(obj: &Value) -> Option<Content> {
        let id = obj["id"].as_str()?;
        let name = obj["name"].as_str().unwrap_or(id);
        let mut raw = RawResource::new(format!("ollie://blob/{id}"), name);
        if let Some(mime) = obj["mime_type"].as_str().or_else(|| obj["content_type"].as_str()) {
            raw = raw.with_mime_type(mime);
        }
        if let Some(size) = obj["size"].as_i64() {
            raw = raw.with_size(size.max(0) as u32);
        }
        Some(Content::resource_link(raw))
    }
    match tool {
        "list_blobs" | "search_blobs" => value["items"]
            .as_array()
            .map(|items| items.iter().filter_map(link_from).collect())
            .unwrap_or_default(),
        "get_blob_metadata" => link_from(value).into_iter().collect(),
        // get_blob_url's payload is the URL, not the record — link by id from args.
        "get_blob_url" => args["id"]
            .as_str()
            .map(|id| {
                vec![Content::resource_link(RawResource::new(
                    format!("ollie://blob/{id}"),
                    id,
                ))]
            })
            .unwrap_or_default(),
        _ => vec![],
    }
}

/// Resolve an `ollie://{kind}/{id}` URI to JSON resource contents.
async fn read_ollie_resource(state: &AppState, uri: &str) -> Result<ResourceContents, McpError> {
    let rest = uri
        .strip_prefix("ollie://")
        .ok_or_else(|| McpError::invalid_params(format!("unsupported resource URI: {uri}"), None))?;
    let (kind, id_str) = rest
        .split_once('/')
        .ok_or_else(|| McpError::invalid_params(format!("malformed resource URI: {uri}"), None))?;
    let id = Uuid::parse_str(id_str)
        .map_err(|_| McpError::invalid_params(format!("resource id is not a UUID: {uri}"), None))?;

    let json = match kind {
        "blob" => serde_json::to_value(
            state.db.get_by_id(id).await.map_err(|e| map_record_err(uri, e))?,
        ),
        "load" => serde_json::to_value(
            state.db.get_load_by_id(id).await.map_err(|e| map_record_err(uri, e))?,
        ),
        "trip" => serde_json::to_value(
            super::data::build_trip_detail(state, id).await.map_err(|e| map_record_err(uri, e))?,
        ),
        _ => {
            return Err(McpError::invalid_params(
                format!("unknown resource kind '{kind}' in {uri}"),
                None,
            ))
        }
    }
    .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(ResourceContents::text(json.to_string(), uri).with_mime_type("application/json"))
}

/// Map a record-fetch error to the right MCP code: a genuine miss is
/// `resource_not_found` (-32002), but a transient/internal failure must stay an
/// `internal_error` so clients don't treat a DB outage as a definitive 404.
fn map_record_err(uri: &str, e: crate::error::AppError) -> McpError {
    match e {
        crate::error::AppError::NotFound => {
            McpError::resource_not_found(format!("{uri}: not found"), None)
        }
        other => McpError::internal_error(format!("{uri}: {other}"), None),
    }
}

/// Decode a resources/list pagination cursor into a 0-based offset.
fn resource_offset(cursor: Option<&str>) -> Result<usize, McpError> {
    match cursor {
        None => Ok(0),
        Some(c) => base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(c)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| McpError::invalid_params("invalid cursor", None)),
    }
}

fn encode_offset(offset: usize) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(offset.to_string())
}

// ---------------------------------------------------------------------------
// Completions (MCP `completions` capability, #301)
// ---------------------------------------------------------------------------

/// Maximum suggestions returned for one completion request.
const COMPLETION_LIMIT: usize = 50;

/// Prefix-match suggestions for a reference argument, sourced from existing
/// records. `customer_name` honors a `status` value in the request `context` to
/// narrow to customers with loads in that status (a dependent-argument case).
/// Unknown argument names yield no suggestions.
async fn completion_values(
    state: &AppState,
    req: &CompleteRequestParams,
) -> Result<Vec<String>, McpError> {
    let internal = |e: crate::error::AppError| McpError::internal_error(e.to_string(), None);
    let prefix = req.argument.value.to_lowercase();
    // Validate the context `status` against the LoadStatus enum before it reaches
    // the DB filter — defense-in-depth, since this path bypasses the HTTP-layer
    // enum deserialization other callers get. An unrecognized status is treated as
    // no narrowing rather than passed through.
    let ctx_status = req
        .context
        .as_ref()
        .and_then(|c| c.arguments.as_ref())
        .and_then(|m| m.get("status"))
        .filter(|s| s.parse::<LoadStatus>().is_ok())
        .map(String::as_str);

    // Pull a bounded candidate set, then prefix-filter in memory.
    let candidates: Vec<String> = match req.argument.name.as_str() {
        "customer_name" => state
            .db
            .list_loads(ctx_status, None, &[], None, None, 500, 0)
            .await
            .map_err(internal)?
            .1
            .into_iter()
            .map(|l| l.customer_name)
            .collect(),
        "tag" => state
            .db
            .list(None, &[], 500, 0)
            .await
            .map_err(internal)?
            .1
            .into_iter()
            .flat_map(|b| b.tags)
            .collect(),
        // facility search argument is named `q` (see list_facilities/get_facility).
        "q" => state
            .db
            .list_facilities(None, &[], 500, 0)
            .await
            .map_err(internal)?
            .1
            .into_iter()
            .map(|f| f.name)
            .collect(),
        _ => return Ok(vec![]),
    };

    // Case-insensitive prefix match; BTreeSet dedups + sorts; cap the count.
    let matches: std::collections::BTreeSet<String> = candidates
        .into_iter()
        .filter(|v| prefix.is_empty() || v.to_lowercase().starts_with(&prefix))
        .collect();
    Ok(matches.into_iter().take(COMPLETION_LIMIT).collect())
}

/// Build the rmcp Streamable HTTP service for mounting under `/dispatch/mcp`.
///
/// Stateful mode (the full Streamable HTTP transport): an `initialize` POST opens
/// a session (returned in the `Mcp-Session-Id` response header), and responses
/// stream back as `text/event-stream`. This is what unblocks server→client
/// messages for resource subscriptions (#299) and elicitation (#300).
///
/// `sse_keep_alive` is disabled so each request's response stream terminates as
/// soon as its single JSON-RPC reply is delivered, instead of being held open by
/// periodic pings. DNS-rebinding host allow-listing is disabled because the only
/// client contract is a `Bearer` token over a public domain (no browser/cookie
/// ambient authority), and the default allow-list (loopback only) would 403 every
/// production request.
pub fn mcp_service(state: &AppState) -> StreamableHttpService<OllieMcp, LocalSessionManager> {
    let state = state.clone();
    StreamableHttpService::new(
        move || Ok(OllieMcp::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(None)
            .disable_allowed_hosts(),
    )
}

/// The full tool catalogue as rmcp `Tool`s, derived from the hand-authored
/// `tools_list()` JSON schema so the per-tool input schemas live in one place.
fn tool_catalog() -> Vec<Tool> {
    tools_list()["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| {
            let name = t["name"].as_str()?.to_string();
            let description = t["description"].as_str().unwrap_or("").to_string();
            let mut schema = t["inputSchema"].as_object().cloned().unwrap_or_default();
            if PAGINATED_LIST_TOOLS.contains(&name.as_str()) {
                advertise_cursor(&mut schema);
            }
            let mut tool = Tool::new(name.clone(), description, Arc::new(schema))
                .with_title(title_case(&name))
                .with_annotations(annotations_for(&name));
            if let Some(out) = output_schema_for(&name) {
                tool = tool.with_raw_output_schema(Arc::new(out));
            }
            Some(tool)
        })
        .collect()
}

/// Declared `outputSchema` for high-traffic tools (MCP 2025-06-18), so clients get
/// a machine-checkable contract for `structuredContent` instead of re-parsing free
/// text. Schemas name the guaranteed fields and allow extra ones
/// (`additionalProperties` defaults to true), so they stay forward-compatible as
/// records gain fields. Returns None for tools without a declared output schema.
fn output_schema_for(name: &str) -> Option<serde_json::Map<String, Value>> {
    // Shared wrapper for the paginated/ranked list tools: a page of objects plus
    // counts and (when present) the pagination cursor / truncation flag.
    let list_envelope = || {
        serde_json::json!({
            "type": "object",
            "properties": {
                "items":      { "type": "array", "items": { "type": "object" } },
                "returned":   { "type": "integer" },
                "total":      { "type": "integer" },
                "nextCursor": { "type": "string" },
                "truncated":  { "type": "boolean" }
            },
            "required": ["items", "returned"]
        })
    };
    // A single record always carries at least a UUID `id`.
    let record = || {
        serde_json::json!({
            "type": "object",
            "properties": { "id": { "type": "string", "format": "uuid" } },
            "required": ["id"]
        })
    };
    let schema = match name {
        "list_loads" | "list_trips" | "list_blobs" | "search_blobs" => list_envelope(),
        "get_load" | "get_trip" => record(),
        _ => return None,
    };
    schema.as_object().cloned()
}

/// Cursor-paginated list tools — they accept `cursor` and return `nextCursor`.
const PAGINATED_LIST_TOOLS: &[&str] = &[
    "list_loads",
    "list_trips",
    "list_drivers",
    "list_trucks",
    "list_trailers",
    "list_facilities",
    "list_blobs",
    "list_events",
];

/// Advertise the `cursor` pagination param on a list tool's input schema. The
/// handlers read `cursor` regardless; this just makes it discoverable in
/// tools/list (defined once rather than duplicated across eight schemas).
fn advertise_cursor(schema: &mut serde_json::Map<String, Value>) {
    let props = schema
        .entry("properties")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(props) = props.as_object_mut() {
        props.insert(
            "cursor".to_string(),
            serde_json::json!({
                "type": "string",
                "description": "Opaque pagination cursor from a prior response's nextCursor; omit for the first page. Absence of nextCursor means the list is complete."
            }),
        );
    }
}

/// Behavioral hints for a tool (MCP `annotations`). Advisory only — the server
/// still enforces its own guards; these just let clients gate/auto-confirm calls
/// and let the agent reason about safety. Reviewed against actual tool behavior:
/// `*_doctor` tools can apply repairs, so they are NOT marked read-only.
fn annotations_for(name: &str) -> ToolAnnotations {
    if name.starts_with("list_") || name.starts_with("get_") || name == "search_blobs" {
        // Pure reads. destructive/idempotent hints are meaningless when read-only.
        return ToolAnnotations::from_raw(None, Some(true), None, None, None);
    }
    let destructive = matches!(
        name,
        "delete_blob"
            | "cancel_trip"
            | "unassign_driver"
            | "detach_equipment"
            | "cancel_load"
            | "delete_load"
            | "delete_trip"
            | "delete_driver"
            | "delete_truck"
            | "delete_trailer"
            | "delete_facility"
    );
    // update_* set fields to a target value; dispatch/undispatch converge to a
    // status — re-running with the same args is a no-op.
    let idempotent =
        name.starts_with("update_") || matches!(name, "dispatch_trip" | "undispatch_trip");
    ToolAnnotations::from_raw(
        None,
        Some(false),
        destructive.then_some(true),
        idempotent.then_some(true),
        None,
    )
}

/// `list_loads` -> `List Loads`. Human-friendly display title; `name` stays the
/// programmatic id.
fn title_case(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Serialize a tool handler's payload to a JSON `Value`. The ServerHandler wraps
/// the result into an MCP text content block; handlers just return their data.
fn mcp_content(value: impl Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
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
                        "previous_trip_id": { "type": "string", "format": "uuid" },
                        "blob_ids": { "type": "array", "items": { "type": "string", "format": "uuid" } }
                    }
                }
            },
            {
                "name": "update_trip",
                "description": "Update a trip's notes, blob_ids, and/or previous_trip_id link. Setting previous_trip_id triggers a mileage recompute. Mileage fields cannot be set directly.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "trip_id": { "type": "string", "format": "uuid" },
                        "notes": { "type": "string" },
                        "previous_trip_id": { "type": "string", "format": "uuid" },
                        "blob_ids": { "type": "array", "items": { "type": "string", "format": "uuid" } }
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
                "name": "attach_equipment",
                "description": "Attach a truck and/or trailers to a driver. Trailers are additive (merged with any already attached). Attaching a truck releases the driver's previous truck to available first. Rejected if the driver is inactive or any equipment is on another driver's active (dispatched/in_transit) trip. If the driver has an active trip, the trip's truck/trailers are synced. Pure equipment event — does not change trip status.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "driver_id":   { "type": "string", "format": "uuid" },
                        "truck":       { "type": "string", "format": "uuid" },
                        "trailer_ids": { "type": "array", "items": { "type": "string", "format": "uuid" } }
                    },
                    "required": ["driver_id"]
                }
            },
            {
                "name": "detach_equipment",
                "description": "Detach a driver's truck and/or drop trailers, releasing them to available. Set truck=true to un-seat the truck; pass trailer_ids to drop specific trailers, or all_trailers=true to drop every trailer. Syncs the driver's active trip when present. Pure equipment event — does not change trip status.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "driver_id":    { "type": "string", "format": "uuid" },
                        "truck":        { "type": "boolean", "default": false },
                        "trailer_ids":  { "type": "array", "items": { "type": "string", "format": "uuid" } },
                        "all_trailers": { "type": "boolean", "default": false }
                    },
                    "required": ["driver_id"]
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
                        "notes":       { "type": "string" },
                        "blob_ids":    { "type": "array", "items": { "type": "string", "format": "uuid" } }
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
                        "notes":       { "type": "string" },
                        "blob_ids":    { "type": "array", "items": { "type": "string", "format": "uuid" } }
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
                        "notes":        { "type": "string" },
                        "blob_ids":     { "type": "array", "items": { "type": "string", "format": "uuid" } }
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
                        "notes":        { "type": "string" },
                        "blob_ids":     { "type": "array", "items": { "type": "string", "format": "uuid" } }
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
            },
            {
                "name": "upload_blob",
                "description": "Upload a file (PDF, scan, contract, etc.) to the blob store. Returns a short-lived presigned URL — do NOT stream file bytes through this tool call. POST the raw file bytes to the returned url with a Content-Type header (optional query params name and tags, comma-separated), e.g. curl -X POST --data-binary @doc.pdf -H 'Content-Type: application/pdf' '<url>&name=doc.pdf'. The HTTP response is the created blob record; use its id in the blob_ids of create_load/update_load, create_facility/update_facility, create_trip/update_trip, create_driver/update_driver (admin API), create_truck/update_truck, and create_trailer/update_trailer. Requires OLLIE_PUBLIC_BASE_URL to be configured.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "expires_in_seconds": { "type": "integer", "minimum": 1, "description": "TTL for the URL; clamped to the server max (default 300s)." }
                    }
                }
            },
            {
                "name": "get_blob_url",
                "description": "Mint a short-lived presigned GET URL for downloading a blob's bytes. GET the url to retrieve the file; for large files stream to disk (e.g. curl -o out.pdf '<url>') rather than reading into context. Requires OLLIE_PUBLIC_BASE_URL to be configured.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "format": "uuid" },
                        "expires_in_seconds": { "type": "integer", "minimum": 1, "description": "TTL for the URL; clamped to the server max (default 300s)." }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "get_blob_metadata",
                "description": "Fetch a blob's metadata (no bytes) plus a reverse lookup of what references it: attached_to.loads, attached_to.facilities, attached_to.trips, attached_to.drivers, attached_to.trucks, and attached_to.trailers.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "format": "uuid" }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "list_blobs",
                "description": "List blob metadata. Optional filters: name (substring), tag (exact), content_type (exact MIME match), limit (default 100, max 1000). Response includes `total` (count for the name/tag filter) and `truncated` (true when more results exist than were returned — for content_type queries this means the scan window was saturated).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name":         { "type": "string" },
                        "tag":          { "type": "string" },
                        "content_type": { "type": "string" },
                        "limit":        { "type": "integer", "minimum": 1, "maximum": 1000 }
                    }
                }
            },
            {
                "name": "delete_blob",
                "description": "Delete a blob. By default fails if the blob is referenced by any load or facility; pass force=true to delete anyway. Storage bytes are removed only when no other blob record shares the same checksum. Returns { deleted, was_attached }.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id":    { "type": "string", "format": "uuid" },
                        "force": { "type": "boolean", "default": false }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "search_blobs",
                "description": "SEMANTIC search over blobs by meaning, using vector similarity over Ollama embeddings of each blob's summary — use this for natural-language queries like 'rate con mentioning hazmat detention'. (Contrast list_blobs, which only does literal name-substring and exact-tag matching.) Returns ranked BlobListItems each with a `score` (higher = closer), best match first. Optional name/tag pre-filters and limit (default 10, max 100).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Natural-language search text." },
                        "name":  { "type": "string", "description": "Optional name-substring pre-filter." },
                        "tag":   { "type": "string", "description": "Optional exact-tag pre-filter." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 10 }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "delete_load",
                "description": "Delete a load record. Fails if the load has any active trips — cancel or complete them first. Returns { deleted: true }.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "id": { "type": "string", "format": "uuid" } },
                    "required": ["id"]
                }
            },
            {
                "name": "delete_trip",
                "description": "Delete a trip. Active trips are soft-cancelled; already-cancelled trips are hard-deleted. Blocked if the trip is in_transit, delivered, or completed. Returns { deleted: true }.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "id": { "type": "string", "format": "uuid" } },
                    "required": ["id"]
                }
            },
            {
                "name": "delete_driver",
                "description": "Soft-delete a driver (status → inactive) and invalidate any outstanding driver JWTs. Returns { deleted: true }.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "id": { "type": "string", "format": "uuid" } },
                    "required": ["id"]
                }
            },
            {
                "name": "delete_truck",
                "description": "Soft-delete a truck (status → inactive). Returns { deleted: true }.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "truck_id": { "type": "string", "format": "uuid" } },
                    "required": ["truck_id"]
                }
            },
            {
                "name": "delete_trailer",
                "description": "Soft-delete a trailer (status → inactive). Returns { deleted: true }.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "trailer_id": { "type": "string", "format": "uuid" } },
                    "required": ["trailer_id"]
                }
            },
            {
                "name": "delete_facility",
                "description": "Delete a facility record. Fails if the facility is referenced by one or more loads. Returns { deleted: true }.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "facility_id": { "type": "string", "format": "uuid" } },
                    "required": ["facility_id"]
                }
            },
            {
                "name": "invoice_load",
                "description": "Transition a load to `invoiced`, optionally recording an invoice number and date. Returns the dispatcher-enriched load detail.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id":             { "type": "string", "format": "uuid" },
                        "invoice_number": { "type": "string" },
                        "invoice_date":   { "type": "string" }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "cancel_load",
                "description": "Transition a load to `cancelled`, optionally recording a reason. Returns the dispatcher-enriched load detail.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id":     { "type": "string", "format": "uuid" },
                        "reason": { "type": "string" }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "settle_load",
                "description": "Transition a load to `settled`. Returns the dispatcher-enriched load detail.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "id": { "type": "string", "format": "uuid" } },
                    "required": ["id"]
                }
            },
            {
                "name": "set_driver_pin",
                "description": "Set a driver's portal PIN (4–6 numeric digits). Invalidates any outstanding driver JWTs. Returns { pin_set: true }.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id":  { "type": "string", "format": "uuid" },
                        "pin": { "type": "string", "description": "4–6 numeric digits." }
                    },
                    "required": ["id", "pin"]
                }
            },
            {
                "name": "create_driver",
                "description": "Create a new driver. Defaults status to `available`; assigns the default terminal when terminal_id is omitted.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name":                   { "type": "string" },
                        "phone":                  { "type": "string" },
                        "email":                  { "type": "string" },
                        "license_number":         { "type": "string" },
                        "license_state":          { "type": "string" },
                        "license_expiry":         { "type": "string" },
                        "notes":                  { "type": "string" },
                        "blob_ids":               { "type": "array", "items": { "type": "string", "format": "uuid" } },
                        "terminal_id":            { "type": "string", "format": "uuid" },
                        "loaded_rate_per_mile":   { "type": "number" },
                        "deadhead_rate_per_mile": { "type": "number" },
                        "extra_stop_fee":         { "type": "number" },
                        "detention_rate_per_hour":{ "type": "number" },
                        "free_dwell_minutes":     { "type": "integer" }
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "update_driver",
                "description": "Update a driver's fields. `status` is not settable here — drivers transition via the trip lifecycle.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id":                     { "type": "string", "format": "uuid" },
                        "name":                   { "type": "string" },
                        "phone":                  { "type": "string" },
                        "email":                  { "type": "string" },
                        "license_number":         { "type": "string" },
                        "license_state":          { "type": "string" },
                        "license_expiry":         { "type": "string" },
                        "notes":                  { "type": "string" },
                        "blob_ids":               { "type": "array", "items": { "type": "string", "format": "uuid" } },
                        "terminal_id":            { "type": "string", "format": "uuid" },
                        "loaded_rate_per_mile":   { "type": "number" },
                        "deadhead_rate_per_mile": { "type": "number" },
                        "extra_stop_fee":         { "type": "number" },
                        "detention_rate_per_hour":{ "type": "number" },
                        "free_dwell_minutes":     { "type": "integer" }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "list_users",
                "description": "List fleet users (the dispatchers-with-roles population). Requires users:read (owner/fleet_manager). Never returns password hashes.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "get_user",
                "description": "Get a single fleet user by UUID. Requires users:read. Never returns the password hash.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "id": { "type": "string", "format": "uuid" } },
                    "required": ["id"]
                }
            },
            {
                "name": "create_user",
                "description": "Create a fleet user with a role (fleet_manager or dispatcher) and optional extra_scopes. Requires users:write. role=owner is rejected — ownership is established by bootstrap or transfer.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "email":        { "type": "string" },
                        "name":         { "type": "string" },
                        "password":     { "type": "string" },
                        "role":         { "type": "string", "enum": ["fleet_manager","dispatcher"] },
                        "extra_scopes": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["email", "name", "password", "role"]
                }
            },
            {
                "name": "update_user",
                "description": "Update a user's name, status, role, and/or extra_scopes. Requires users:write. Owner-protection applies: the owner cannot be demoted/deactivated except via ownership transfer; setting a different user's role to owner is an ownership transfer permitted only when the caller is the current owner.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id":           { "type": "string", "format": "uuid" },
                        "name":         { "type": "string" },
                        "status":       { "type": "string", "enum": ["active","inactive"] },
                        "role":         { "type": "string", "enum": ["owner","fleet_manager","dispatcher"] },
                        "extra_scopes": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "reset_user_password",
                "description": "Reset a user's password, invalidating their outstanding JWTs (token_version bump). Requires users:write. Returns { password_reset: true }.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id":       { "type": "string", "format": "uuid" },
                        "password": { "type": "string" }
                    },
                    "required": ["id", "password"]
                }
            },
            {
                "name": "delete_user",
                "description": "Deactivate a user (status → inactive) and revoke their access. Requires users:delete. The only owner cannot be deactivated — transfer ownership first. Returns { deleted: true }.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "id": { "type": "string", "format": "uuid" } },
                    "required": ["id"]
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
// Tool dispatch
// ---------------------------------------------------------------------------

/// Dispatch a `tools/call` by name to the matching tool shim. Returns the raw
/// JSON payload (the ServerHandler wraps it into an MCP content block). An unknown
/// tool is a protocol fault (`ToolError::Unknown`); any shim error is a domain
/// failure (`ToolError::Domain`) surfaced to the model as an isError result.
async fn handle_tool_call(
    state: &AppState,
    name: &str,
    args: &Value,
    scopes: &[String],
    caller_id: Option<Uuid>,
) -> Result<Value, ToolError> {
    let result: Result<Value, String> = match name {
        "list_loads" => tool_list_loads(state, args).await,
        "get_load" => tool_get_load(state, args).await,
        "create_load" => tool_create_load(state, args).await,
        "update_load" => tool_update_load(state, args).await,
        "list_trips" => tool_list_trips(state, args).await,
        "get_trip" => tool_get_trip(state, args).await,
        "create_trip" => tool_create_trip(state, args).await,
        "update_trip" => tool_update_trip(state, args).await,
        "recalculate_trip_miles" => tool_recalculate_trip_miles(state, args, scopes).await,
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
        "attach_equipment" => tool_attach_equipment(state, args).await,
        "detach_equipment" => tool_detach_equipment(state, args).await,
        "list_trucks" => tool_list_trucks(state, args).await,
        "get_truck" => tool_get_truck(state, args).await,
        "create_truck" => tool_create_truck(state, args).await,
        "update_truck" => tool_update_truck(state, args).await,
        "list_trailers" => tool_list_trailers(state, args).await,
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
        "upload_blob" => tool_upload_blob(state, args).await,
        "get_blob_url" => tool_get_blob_url(state, args).await,
        "get_blob_metadata" => tool_get_blob_metadata(state, args).await,
        "list_blobs" => tool_list_blobs(state, args).await,
        "search_blobs" => tool_search_blobs(state, args).await,
        "delete_blob" => tool_delete_blob(state, args).await,
        "delete_load" => tool_delete_load(state, args).await,
        "delete_trip" => tool_delete_trip(state, args).await,
        "delete_driver" => tool_delete_driver(state, args).await,
        "delete_truck" => tool_delete_truck(state, args).await,
        "delete_trailer" => tool_delete_trailer(state, args).await,
        "delete_facility" => tool_delete_facility(state, args).await,
        "invoice_load" => tool_invoice_load(state, args).await,
        "cancel_load" => tool_cancel_load(state, args).await,
        "settle_load" => tool_settle_load(state, args).await,
        "set_driver_pin" => tool_set_driver_pin(state, args).await,
        "create_driver" => tool_create_driver(state, args).await,
        "update_driver" => tool_update_driver(state, args).await,
        "list_users" => tool_list_users(state).await,
        "get_user" => tool_get_user(state, args).await,
        "create_user" => tool_create_user(state, args, scopes, caller_id).await,
        "update_user" => tool_update_user(state, args, scopes, caller_id).await,
        "reset_user_password" => tool_reset_user_password(state, args, caller_id).await,
        "delete_user" => tool_delete_user(state, args).await,
        _ => return Err(ToolError::Unknown),
    };
    result.map_err(ToolError::Domain)
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Cursor pagination
//
// List tools accept an opaque `cursor` and return `nextCursor` when more results
// exist, so an agent can page through the full set deterministically. The cursor
// is the URL-safe base64 of the next 0-based offset; absence of `nextCursor`
// reliably signals end-of-list. Without this, list tools silently truncated at
// the first page (the latent bug in #296).
// ---------------------------------------------------------------------------

/// Default page size for cursor-paginated list tools (tools with their own
/// `limit` arg pass that instead).
const PAGE_SIZE: usize = 100;

/// Decode the opaque `cursor` arg into a 0-based offset. Absent → first page.
fn cursor_offset(args: &Value) -> Result<usize, String> {
    match args["cursor"].as_str() {
        None => Ok(0),
        Some(c) => base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(c)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| "invalid cursor".to_string()),
    }
}

/// Assemble a paginated list payload: the page `items`, how many were `returned`,
/// the full match `total`, and `nextCursor` when records remain past this page.
fn paged(items: impl Serialize, returned: usize, total: usize, offset: usize) -> Value {
    let mut obj = serde_json::json!({
        "items": items,
        "returned": returned,
        "total": total,
    });
    let next = offset + returned;
    if next < total {
        let cursor = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(next.to_string());
        obj["nextCursor"] = Value::String(cursor);
    }
    obj
}

/// Paginate an already-materialized, fully-filtered list in memory (for tools
/// whose filtering happens after the DB fetch). Consumes the list and returns
/// (page, returned, total).
fn paginate_slice<T>(all: Vec<T>, offset: usize, page: usize) -> (Vec<T>, usize, usize) {
    let total = all.len();
    let items: Vec<T> = all.into_iter().skip(offset).take(page).collect();
    let returned = items.len();
    (items, returned, total)
}

async fn tool_list_loads(state: &AppState, args: &Value) -> Result<Value, String> {
    let status = args["status"].as_str();
    let offset = cursor_offset(args)?;

    let (total, items) = state.db.list_loads(
        status,
        None, // customer
        &[],  // tags
        None, // from
        None, // to
        PAGE_SIZE,
        offset,
    ).await.map_err(|e| e.to_string())?;

    let returned = items.len();
    Ok(mcp_content(paged(items, returned, total, offset)))
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
        pay_period_start: args["pay_period_start"].as_str().map(|s| s.to_string()),
        pay_period_end: args["pay_period_end"].as_str().map(|s| s.to_string()),
    };
    let offset = cursor_offset(args)?;
    // build_trip_list_items materializes the full matching set (a pre-existing
    // shared-path constraint — it has no limit/offset; the REST trips list uses it
    // too), so this is O(N) per page regardless of page size. Acceptable for now;
    // pushing limit/offset into that helper is the proper fix when trip counts grow.
    let all = super::data::build_trip_list_items(state, q).await
        .map_err(|e| e.to_string())?;
    let (page, returned, total) = paginate_slice(all, offset, PAGE_SIZE);
    Ok(mcp_content(paged(page, returned, total, offset)))
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

    // Create via the shared writer, which returns the created record — no
    // re-fetch (that races under concurrent creates).
    let record = crate::api::trips::apply_trip_create(state, req).await
        .map_err(|e| e.to_string())?;
    let detail = super::data::build_trip_detail(state, record.id).await
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

async fn tool_recalculate_trip_miles(
    state: &AppState,
    args: &Value,
    scopes: &[String],
) -> Result<Value, String> {
    use super::trip_writes::{recalculate_miles_handler, RecalculateMilesBody};
    let trip_id = parse_uuid(args, "trip_id")?;
    let force = args["force"].as_bool().unwrap_or(false);

    let body = Some(Json(RecalculateMilesBody { force }));
    // call_tool already verified trips:write for this caller; the handler re-checks
    // its scope internally, so pass the caller's effective scopes through.
    let _resp = recalculate_miles_handler(
        axum::extract::State(state.clone()),
        axum::Extension(claims_with_scopes(scopes)),
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
    let offset = cursor_offset(args)?;
    let (total, items) = state.db.list_drivers(status, PAGE_SIZE, offset)
        .await.map_err(|e| e.to_string())?;
    let returned = items.len();
    Ok(mcp_content(paged(items, returned, total, offset)))
}

async fn tool_get_driver(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let record = state.db.get_driver_by_id(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_attach_equipment(state: &AppState, args: &Value) -> Result<Value, String> {
    let driver_id = parse_uuid(args, "driver_id")?;
    let mut body = args.clone();
    if let Value::Object(map) = &mut body {
        map.remove("driver_id");
    }
    let change = super::driver_writes::apply_attach_equipment(state, driver_id, body)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(change))
}

async fn tool_detach_equipment(state: &AppState, args: &Value) -> Result<Value, String> {
    let driver_id = parse_uuid(args, "driver_id")?;
    let mut body = args.clone();
    if let Value::Object(map) = &mut body {
        map.remove("driver_id");
    }
    let change = super::driver_writes::apply_detach_equipment(state, driver_id, body)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(change))
}

async fn tool_list_trucks(state: &AppState, args: &Value) -> Result<Value, String> {
    let offset = cursor_offset(args)?;
    let (total, items) = state.db.list_trucks(None, PAGE_SIZE, offset)
        .await.map_err(|e| e.to_string())?;
    let returned = items.len();
    Ok(mcp_content(paged(items, returned, total, offset)))
}

async fn tool_list_trailers(state: &AppState, args: &Value) -> Result<Value, String> {
    let offset = cursor_offset(args)?;
    let (total, items) = state.db.list_trailers(None, None, PAGE_SIZE, offset)
        .await.map_err(|e| e.to_string())?;
    let returned = items.len();
    Ok(mcp_content(paged(items, returned, total, offset)))
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

    const EVENTS_PAGE: usize = 20;
    let offset = cursor_offset(args)?;
    let (total, records) = state.db.query_events(
        entity_id,
        None,
        None,
        None,
        None,
        EVENTS_PAGE,
        offset,
    ).await.map_err(|e| e.to_string())?;

    let items: Vec<crate::models::EventResponse> = records.into_iter().map(crate::models::EventResponse::from).collect();
    let returned = items.len();
    Ok(mcp_content(paged(items, returned, total, offset)))
}

// ---------------------------------------------------------------------------
// Facilities — list / get / create / update share the dispatcher write helpers
// in `facility_writes` so HTTP and MCP enforce the same validation + side
// effects (geocode queue, manual-coords override).
// ---------------------------------------------------------------------------

async fn tool_list_facilities(state: &AppState, args: &Value) -> Result<Value, String> {
    let limit = args["limit"].as_u64().map(|n| n as usize).unwrap_or(PAGE_SIZE).min(1000);
    let q = args["q"].as_str().map(|s| s.to_string());
    let offset = cursor_offset(args)?;

    let (_total, items) = state.db.list_facilities(None, &[], 1000, 0)
        .await.map_err(|e| e.to_string())?;

    // `q` filtering happens after the DB fetch, so paginate the filtered set in
    // memory; `limit` is the page size and the cursor offsets into it.
    let matched: Vec<_> = if let Some(needle) = q.as_deref().filter(|s| !s.is_empty()) {
        let needle = needle.to_lowercase();
        items.into_iter()
            .filter(|f| {
                f.name.to_lowercase().contains(&needle)
                    || f.address.to_lowercase().contains(&needle)
            })
            .collect()
    } else {
        items
    };
    let (page, returned, total) = paginate_slice(matched, offset, limit);
    Ok(mcp_content(paged(page, returned, total, offset)))
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

// ---------------------------------------------------------------------------
// Blob store tools.
//
// File bytes never traverse MCP. All transfers go through presigned URLs minted
// here and served by the token-authenticated routes in
// `blobs::presigned_{upload,download}`.
// ---------------------------------------------------------------------------

/// Clamp a caller-requested TTL to [1, server max], defaulting when omitted.
fn resolve_presign_ttl(state: &AppState, args: &Value) -> u64 {
    args.get("expires_in_seconds")
        .and_then(|v| v.as_u64())
        .unwrap_or(state.config.blob_presign_ttl_secs)
        .clamp(1, state.config.blob_presign_max_ttl_secs)
}

fn require_base_url(state: &AppState) -> Result<String, String> {
    let base = &state.config.public_base_url;
    if base.is_empty() {
        Err("OLLIE_PUBLIC_BASE_URL is not configured, so presigned URLs cannot be built. \
             Ask an operator to set OLLIE_PUBLIC_BASE_URL."
            .to_string())
    } else {
        Ok(base.clone())
    }
}

fn unix_to_rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

async fn tool_upload_blob(state: &AppState, args: &Value) -> Result<Value, String> {
    let base = require_base_url(state)?;
    let ttl = resolve_presign_ttl(state, args);
    let (token, exp) = blob_links::mint_token(&state.config.dispatcher_jwt_secret, BlobUrlOp::Post, None, ttl)
        .map_err(|e| e.to_string())?;
    let url = blob_links::upload_url(&base, &token);
    Ok(mcp_content(serde_json::json!({
        "upload_url": url,
        "method": "POST",
        "expires_at": unix_to_rfc3339(exp),
        "max_bytes": crate::api::blobs::PRESIGNED_UPLOAD_MAX_BYTES,
        "instructions": "POST the raw file bytes to upload_url with a Content-Type header set to the file's MIME type. \
            Optional query params: name, tags (comma-separated). \
            Example: curl -X POST --data-binary @doc.pdf -H 'Content-Type: application/pdf' '<upload_url>&name=doc.pdf&tags=invoice,2026'. \
            The JSON response is the created blob record — use its id in blob_ids."
    })))
}

async fn tool_get_blob_url(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let base = require_base_url(state)?;
    // Confirm the blob exists before handing out a URL for it.
    state.db.get_by_id(id).await.map_err(|e| e.to_string())?;
    let ttl = resolve_presign_ttl(state, args);
    let (token, exp) = blob_links::mint_token(&state.config.dispatcher_jwt_secret, BlobUrlOp::Get, Some(id), ttl)
        .map_err(|e| e.to_string())?;
    let url = blob_links::download_url(&base, id, &token);
    Ok(mcp_content(serde_json::json!({
        "url": url,
        "method": "GET",
        "expires_at": unix_to_rfc3339(exp),
        "instructions": "GET this URL to download the raw bytes. For large files, stream to disk \
            (e.g. curl -o out.pdf '<url>') rather than reading the payload into context."
    })))
}

async fn tool_get_blob_metadata(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let record = state.db.get_by_id(id).await.map_err(|e| e.to_string())?;
    let loads = state.db.loads_referencing_blob(id).await.map_err(|e| e.to_string())?;
    let facilities = state.db.facilities_referencing_blob(id).await.map_err(|e| e.to_string())?;
    let trips = state.db.trips_referencing_blob(id).await.map_err(|e| e.to_string())?;
    let drivers = state.db.drivers_referencing_blob(id).await.map_err(|e| e.to_string())?;
    let trucks = state.db.trucks_referencing_blob(id).await.map_err(|e| e.to_string())?;
    let trailers = state.db.trailers_referencing_blob(id).await.map_err(|e| e.to_string())?;
    let mut value = serde_json::to_value(&record).map_err(|e| e.to_string())?;
    value["attached_to"] = serde_json::json!({
        "loads": loads, "facilities": facilities, "trips": trips,
        "drivers": drivers, "trucks": trucks, "trailers": trailers,
    });
    Ok(mcp_content(value))
}

async fn tool_list_blobs(state: &AppState, args: &Value) -> Result<Value, String> {
    let limit = args["limit"].as_u64().map(|n| n as usize).unwrap_or(PAGE_SIZE).min(1000);
    let name = args["name"].as_str();
    let content_type = args["content_type"].as_str();
    let tags: Vec<String> = args["tag"].as_str().map(|t| vec![t.to_string()]).unwrap_or_default();
    let offset = cursor_offset(args)?;

    match content_type {
        // No content_type filter: the DB applies name/tag at the source, so cursor
        // pagination is exact — nextCursor when records remain past this page.
        None => {
            let (total, items) = state.db.list(name, &tags, limit, offset)
                .await.map_err(|e| e.to_string())?;
            let returned = items.len();
            Ok(mcp_content(paged(items, returned, total, offset)))
        }
        // content_type isn't a DB-level filter, so it's applied in memory over a
        // FIXED scan window from the start (independent of cursor depth, to bound
        // memory). We paginate the matches found within that window; `truncated`
        // flags that more matches may lie beyond it (unreachable by cursor here),
        // so a paging agent knows the MIME-filtered view is incomplete.
        Some(ct) => {
            let window = limit.max(1000);
            let (_total, items) = state.db.list(name, &tags, window, 0)
                .await.map_err(|e| e.to_string())?;
            let scanned = items.len();
            let matched: Vec<_> = items.into_iter().filter(|i| i.mime_type == ct).collect();
            let (page, returned, matched_total) = paginate_slice(matched, offset, limit);
            let mut obj = paged(page, returned, matched_total, offset);
            obj["truncated"] = Value::Bool(scanned >= window);
            Ok(mcp_content(obj))
        }
    }
}

/// Semantic blob search — a thin shim over the same embedding + vector-search path
/// the REST `GET /blobs?s=` endpoint uses (blobs.rs). The query is embedded via
/// Ollama, then matched against blob-summary vectors; results carry a similarity
/// `score`. Empty queries are rejected before touching Ollama.
async fn tool_search_blobs(state: &AppState, args: &Value) -> Result<Value, String> {
    let query = args["query"].as_str().unwrap_or("").trim();
    if query.is_empty() {
        return Err("search_blobs requires a non-empty `query`".to_string());
    }
    let limit = args["limit"].as_u64().map(|n| n as usize).unwrap_or(10).clamp(1, 100);
    let name = args["name"].as_str();
    let tags: Vec<String> = args["tag"].as_str().map(|t| vec![t.to_string()]).unwrap_or_default();

    let embedding = crate::ai::embed::embed_text(&state.ai, query)
        .await
        .map_err(|e| e.to_string())?;
    let items = state.db.search(embedding, name, &tags, limit)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "returned": items.len(), "items": items })))
}

async fn tool_delete_blob(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let force = args["force"].as_bool().unwrap_or(false);

    let record = state.db.get_by_id(id).await.map_err(|e| e.to_string())?;

    let attached_to_load = state.db.any_load_references_blob(id).await.map_err(|e| e.to_string())?;
    let attached_to_facility = state.db.any_facility_references_blob(id).await.map_err(|e| e.to_string())?;
    let attached_to_trip = state.db.any_trip_references_blob(id).await.map_err(|e| e.to_string())?;
    let attached_to_driver = state.db.any_driver_references_blob(id).await.map_err(|e| e.to_string())?;
    let attached_to_truck = state.db.any_truck_references_blob(id).await.map_err(|e| e.to_string())?;
    let attached_to_trailer = state.db.any_trailer_references_blob(id).await.map_err(|e| e.to_string())?;
    let was_attached = attached_to_load || attached_to_facility || attached_to_trip
        || attached_to_driver || attached_to_truck || attached_to_trailer;

    if was_attached && !force {
        return Err(format!(
            "blob {id} is referenced by one or more loads/facilities/trips/drivers/trucks/trailers; \
             pass force=true to delete anyway"
        ));
    }

    // Delete the DB record FIRST, then re-count by checksum. LanceDB has no
    // transactions, so this ordering is what makes concurrent delete-vs-upload safe:
    // if a concurrent ingest added another record for the same checksum (its storage
    // write is a dedup no-op), the post-delete recount sees it and we keep the bytes.
    // Deleting the row before the bytes also means a mid-operation failure orphans a
    // file (recoverable) rather than leaving a record pointing at deleted bytes.
    state.db.delete_by_id(id).await.map_err(|e| e.to_string())?;
    let remaining = state.db.count_by_checksum(&record.checksum).await.map_err(|e| e.to_string())?;
    if remaining == 0 {
        state.store.delete(&record.checksum).await.map_err(|e| e.to_string())?;
        let extract_base = std::path::Path::new(&state.config.extract_store_path);
        if let Err(e) = crate::storage::extract_store::delete_extract(extract_base, &record.checksum).await {
            tracing::warn!("failed to delete extract cache for {}: {e}", record.checksum);
        }
    }
    Ok(mcp_content(serde_json::json!({ "deleted": true, "was_attached": was_attached })))
}

// ---------------------------------------------------------------------------
// Dispatch-parity write tools (#330) — delete / lifecycle / driver-admin tools
// that mirror the dispatcher REST handlers, reusing the same DbClient ops and
// apply_* helpers so HTTP and MCP stay in lockstep.
// ---------------------------------------------------------------------------

async fn tool_delete_load(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    state.db.get_load_by_id(id).await.map_err(|e| e.to_string())?;
    let active = state.db.count_active_trips_for_load(id).await.map_err(|e| e.to_string())?;
    if active > 0 {
        return Err(format!("load has {active} active trip(s); cancel or complete them first"));
    }
    state.db.delete_load_by_id(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "deleted": true })))
}

async fn tool_delete_trip(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    state.db.delete_trip(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "deleted": true })))
}

async fn tool_delete_driver(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    super::driver_writes::apply_driver_delete(state, id).await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "deleted": true })))
}

async fn tool_delete_truck(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "truck_id")?;
    state.db.soft_delete_truck(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "deleted": true })))
}

async fn tool_delete_trailer(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "trailer_id")?;
    state.db.soft_delete_trailer(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "deleted": true })))
}

async fn tool_delete_facility(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "facility_id")?;
    state.db.get_facility_by_id(id).await.map_err(|e| e.to_string())?;
    if state.db.any_load_references_facility(id).await.map_err(|e| e.to_string())? {
        return Err("facility is referenced by one or more loads and cannot be deleted".to_string());
    }
    state.db.delete_facility_by_id(id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "deleted": true })))
}

async fn tool_invoice_load(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let invoice_number = args["invoice_number"].as_str().map(|s| s.to_string());
    let invoice_date = args["invoice_date"].as_str().map(|s| s.to_string());
    let record = state.db.transition_load_status(
        id, LoadStatus::Invoiced, invoice_number, invoice_date, None,
    ).await.map_err(|e| e.to_string())?;
    let detail = super::data::build_load_detail(state, record).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(detail))
}

async fn tool_cancel_load(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let reason = args["reason"].as_str().map(|s| s.to_string());
    let record = state.db.transition_load_status(
        id, LoadStatus::Cancelled, None, None, reason,
    ).await.map_err(|e| e.to_string())?;
    let detail = super::data::build_load_detail(state, record).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(detail))
}

async fn tool_settle_load(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let record = state.db.transition_load_status(
        id, LoadStatus::Settled, None, None, None,
    ).await.map_err(|e| e.to_string())?;
    let detail = super::data::build_load_detail(state, record).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(detail))
}

async fn tool_set_driver_pin(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let pin = args["pin"].as_str().ok_or("missing or non-string field 'pin'")?.to_string();
    super::driver_writes::apply_set_driver_pin(state, id, pin)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "pin_set": true })))
}

async fn tool_create_driver(state: &AppState, args: &Value) -> Result<Value, String> {
    use crate::models::CreateDriverRequest;
    let req: CreateDriverRequest = serde_json::from_value(args.clone())
        .map_err(|e| format!("invalid create_driver arguments: {e}"))?;
    let record = super::driver_writes::apply_driver_create(state, req)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_update_driver(state: &AppState, args: &Value) -> Result<Value, String> {
    use crate::models::UpdateDriverRequest;
    let id = parse_uuid(args, "id")?;
    let mut body = args.clone();
    if let Value::Object(map) = &mut body {
        map.remove("id");
    }
    let req: UpdateDriverRequest = serde_json::from_value(body)
        .map_err(|e| format!("invalid update_driver arguments: {e}"))?;
    let record = super::driver_writes::apply_driver_patch(state, id, req)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

// --- Users management (#331) ---

/// Recover the caller's current role from the DB by their dispatcher id, so the
/// Users tools enforce owner-only rules identically to the HTTP surface. Falls
/// back to the least-privileged `Dispatcher` when the caller is unidentified.
async fn caller_role_from_id(
    state: &AppState,
    caller_id: Option<Uuid>,
) -> crate::models::permission::Role {
    match caller_id {
        Some(cid) => match state.db.get_dispatcher_by_id(cid).await {
            Ok(r) => r.role,
            Err(_) => crate::models::permission::Role::Dispatcher,
        },
        None => crate::models::permission::Role::Dispatcher,
    }
}

async fn tool_list_users(state: &AppState) -> Result<Value, String> {
    let users = super::users::apply_list_users(state).await.map_err(|e| e.to_string())?;
    let returned = users.len();
    Ok(mcp_content(serde_json::json!({ "users": users, "returned": returned })))
}

async fn tool_get_user(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let record = super::users::apply_get_user(state, id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_create_user(
    state: &AppState,
    args: &Value,
    scopes: &[String],
    caller_id: Option<Uuid>,
) -> Result<Value, String> {
    let req: super::users::CreateUserRequest = serde_json::from_value(args.clone())
        .map_err(|e| format!("invalid create_user arguments: {e}"))?;
    let caller_role = caller_role_from_id(state, caller_id).await;
    let record = super::users::apply_create_user(state, scopes, caller_role, req)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_update_user(
    state: &AppState,
    args: &Value,
    scopes: &[String],
    caller_id: Option<Uuid>,
) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let mut body = args.clone();
    if let Value::Object(map) = &mut body {
        map.remove("id");
    }
    let req: super::users::UpdateUserRequest = serde_json::from_value(body)
        .map_err(|e| format!("invalid update_user arguments: {e}"))?;
    let caller_role = caller_role_from_id(state, caller_id).await;
    let record =
        super::users::apply_update_user(state, scopes, caller_id, caller_role, id, req)
            .await
            .map_err(|e| e.to_string())?;
    Ok(mcp_content(record))
}

async fn tool_reset_user_password(
    state: &AppState,
    args: &Value,
    caller_id: Option<Uuid>,
) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    let password = args["password"].as_str().ok_or("missing or non-string field 'password'")?.to_string();
    let caller_role = caller_role_from_id(state, caller_id).await;
    super::users::apply_reset_password(state, caller_role, id, password)
        .await
        .map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "password_reset": true })))
}

async fn tool_delete_user(state: &AppState, args: &Value) -> Result<Value, String> {
    let id = parse_uuid(args, "id")?;
    super::users::apply_delete_user(state, id).await.map_err(|e| e.to_string())?;
    Ok(mcp_content(serde_json::json!({ "deleted": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ai::OllamaClient, config::Config, db::DbClient, routing::RoutingClient,
        storage::BlobStore,
    };
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn test_state() -> (AppState, TempDir, TempDir) {
        let blob_dir = TempDir::new().unwrap();
        let db_dir = TempDir::new().unwrap();
        std::env::set_var("ADMIN_API_KEY", "test-secret");
        std::env::set_var("DRIVER_JWT_SECRET", "test-driver-jwt-secret-that-is-long-enough");
        std::env::set_var("DISPATCHER_JWT_SECRET", "test-dispatcher-jwt-secret-that-is-long-enough");
        std::env::set_var("DRIVER_RP_ID", "localhost");
        std::env::set_var("DRIVER_RP_ORIGIN", "http://localhost:3000");
        let config = Arc::new(Config::from_env().unwrap());
        let db = Arc::new(DbClient::new(db_dir.path().to_str().unwrap(), 4).await.unwrap());
        let store = Arc::new(BlobStore::new(blob_dir.path().to_str().unwrap()));
        let ai = Arc::new(OllamaClient::new(
            "http://localhost:11434",
            "nomic-embed-text",
            "llama3.2",
            "llava",
        ));
        let geocoding = Arc::new(crate::geocoding::GeocodingClient::new());
        let ors = Arc::new(RoutingClient::new(""));
        let (geocoding_tx, _rx) = async_channel::bounded(10);
        let (routing_tx, _rx2) = async_channel::bounded(10);
        let (pipeline_tx, _rx3) = async_channel::bounded(10);
        let rp_origin = webauthn_rs::prelude::Url::parse("http://localhost:3000").unwrap();
        let webauthn = Arc::new(
            webauthn_rs::prelude::WebauthnBuilder::new("localhost", &rp_origin)
                .unwrap()
                .build()
                .unwrap(),
        );
        let auth_challenge_store = Arc::new(dashmap::DashMap::new());
        let reg_challenge_store = Arc::new(dashmap::DashMap::new());
        let state = AppState {
            db,
            store,
            ai,
            geocoding,
            ors,
            pipeline_tx,
            geocoding_tx,
            routing_tx,
            config,
            webauthn,
            auth_challenge_store,
            reg_challenge_store,
        };
        (state, blob_dir, db_dir)
    }

    #[tokio::test]
    async fn get_info_advertises_protocol_server_and_tools() {
        let (state, _b, _d) = test_state().await;
        let info = OllieMcp::new(state).get_info();
        assert_eq!(info.protocol_version, ProtocolVersion::V_2025_06_18);
        assert_eq!(info.server_info.name, "ollie-dispatcher");
        assert!(
            info.capabilities.tools.is_some(),
            "server must advertise tools capability"
        );
    }

    #[test]
    fn tool_catalog_lists_expected_tools() {
        let catalog = tool_catalog();
        assert!(!catalog.is_empty(), "tool catalog must not be empty");
        for expected in ["list_loads", "assign_driver", "list_events"] {
            assert!(
                catalog.iter().any(|t| t.name == expected),
                "tool catalog must contain {expected}"
            );
        }
    }

    #[test]
    fn tool_catalog_carries_titles_and_annotations() {
        let catalog = tool_catalog();
        let find = |n: &str| {
            catalog
                .iter()
                .find(|t| t.name == n)
                .unwrap_or_else(|| panic!("missing tool {n}"))
        };

        // Every tool has a human-friendly title and behavioral annotations.
        for t in &catalog {
            assert!(t.title.is_some(), "{} missing title", t.name);
            assert!(t.annotations.is_some(), "{} missing annotations", t.name);
        }
        assert_eq!(find("list_loads").title.as_deref(), Some("List Loads"));

        // read-only reads.
        let a = find("list_drivers").annotations.as_ref().unwrap();
        assert_eq!(a.read_only_hint, Some(true));
        let a = find("get_load").annotations.as_ref().unwrap();
        assert_eq!(a.read_only_hint, Some(true));

        // destructive mutations.
        let a = find("delete_blob").annotations.as_ref().unwrap();
        assert_eq!(a.read_only_hint, Some(false));
        assert_eq!(a.destructive_hint, Some(true));
        let a = find("cancel_trip").annotations.as_ref().unwrap();
        assert_eq!(a.destructive_hint, Some(true));
        // #330 parity deletes + cancel_load carry the destructive hint.
        for name in [
            "delete_load", "delete_trip", "delete_driver", "delete_truck",
            "delete_trailer", "delete_facility", "cancel_load",
        ] {
            let a = find(name).annotations.as_ref().unwrap();
            assert_eq!(a.destructive_hint, Some(true), "{name} must be destructive");
            assert_eq!(a.read_only_hint, Some(false), "{name} is not read-only");
        }
        // update_driver is idempotent (update_ prefix rule).
        let a = find("update_driver").annotations.as_ref().unwrap();
        assert_eq!(a.idempotent_hint, Some(true));
        // create_driver is a plain additive write.
        let a = find("create_driver").annotations.as_ref().unwrap();
        assert_eq!(a.destructive_hint, None);
        assert_eq!(a.idempotent_hint, None);

        // idempotent mutations.
        let a = find("update_trip").annotations.as_ref().unwrap();
        assert_eq!(a.idempotent_hint, Some(true));
        assert_eq!(a.read_only_hint, Some(false));
        let a = find("dispatch_trip").annotations.as_ref().unwrap();
        assert_eq!(a.idempotent_hint, Some(true));

        // a plain additive write is neither read-only, destructive, nor idempotent.
        let a = find("create_load").annotations.as_ref().unwrap();
        assert_eq!(a.read_only_hint, Some(false));
        assert_eq!(a.destructive_hint, None);
        assert_eq!(a.idempotent_hint, None);

        // *_doctor tools can apply repairs -> must NOT be read-only.
        let a = find("facility_doctor").annotations.as_ref().unwrap();
        assert_eq!(a.read_only_hint, Some(false));
    }

    #[test]
    fn paginated_list_tools_advertise_cursor() {
        let catalog = tool_catalog();
        for name in PAGINATED_LIST_TOOLS {
            let tool = catalog
                .iter()
                .find(|t| &t.name == name)
                .unwrap_or_else(|| panic!("missing {name}"));
            assert!(
                tool.input_schema["properties"].get("cursor").is_some(),
                "{name} should advertise a cursor param"
            );
        }
    }

    #[test]
    fn cursor_roundtrips_and_signals_end_of_list() {
        // Absent cursor = first page.
        assert_eq!(cursor_offset(&json!({})).unwrap(), 0);

        // More results -> a nextCursor that decodes back to the next offset.
        let page = paged(json!([1, 2, 3]), 3, 10, 0);
        assert_eq!(page["returned"], 3);
        assert_eq!(page["total"], 10);
        let cursor = page["nextCursor"].as_str().expect("more results -> nextCursor");
        assert_eq!(cursor_offset(&json!({ "cursor": cursor })).unwrap(), 3);

        // Final page -> no nextCursor (reliable end-of-list signal).
        let last = paged(json!([1]), 1, 4, 3);
        assert!(last.get("nextCursor").is_none(), "exhausted list must omit nextCursor");

        // Garbage cursor is rejected, not silently treated as offset 0.
        assert!(cursor_offset(&json!({ "cursor": "***not-base64***" })).is_err());
    }

    #[test]
    fn paginate_slice_yields_every_item_exactly_once() {
        // Page size 2 over 5 items: pages of [0,1] [2,3] [4], then stop.
        let mut seen = Vec::new();
        let mut offset = 0;
        let mut pages = 0;
        loop {
            let all: Vec<i32> = (0..5).collect();
            let (page, returned, total) = paginate_slice(all, offset, 2);
            assert_eq!(total, 5);
            seen.extend(page);
            pages += 1;
            if offset + returned >= total {
                break;
            }
            offset += returned;
            assert!(pages < 10, "must terminate");
        }
        assert_eq!(seen, vec![0, 1, 2, 3, 4], "every item once, in order");
        assert_eq!(pages, 3);
    }

    #[test]
    fn blob_resource_links_cover_each_tool_shape() {
        let uri_of = |c: &Content| c.as_resource_link().map(|r| r.uri.clone());

        // list_blobs/search_blobs: one link per item, with mime + size.
        let value = json!({ "items": [
            { "id": "11111111-1111-1111-1111-111111111111", "name": "a.pdf", "mime_type": "application/pdf", "size": 12 },
            { "id": "22222222-2222-2222-2222-222222222222", "name": "b.txt", "content_type": "text/plain", "size": 3 },
        ]});
        let links = blob_resource_links("list_blobs", &value, &json!({}));
        assert_eq!(links.len(), 2);
        assert_eq!(uri_of(&links[0]).as_deref(), Some("ollie://blob/11111111-1111-1111-1111-111111111111"));
        let r0 = links[0].as_resource_link().unwrap();
        assert_eq!(r0.mime_type.as_deref(), Some("application/pdf"));
        assert_eq!(r0.size, Some(12));

        // get_blob_metadata: a single link from the record value.
        let rec = json!({ "id": "33333333-3333-3333-3333-333333333333", "name": "c", "mime_type": "image/png", "size": 9 });
        let links = blob_resource_links("get_blob_metadata", &rec, &json!({}));
        assert_eq!(links.len(), 1);
        assert_eq!(uri_of(&links[0]).as_deref(), Some("ollie://blob/33333333-3333-3333-3333-333333333333"));

        // get_blob_url: the distinct args-based path (payload is the URL, not the record).
        let links = blob_resource_links(
            "get_blob_url",
            &json!({ "url": "https://x/y", "method": "GET" }),
            &json!({ "id": "44444444-4444-4444-4444-444444444444" }),
        );
        assert_eq!(links.len(), 1);
        assert_eq!(uri_of(&links[0]).as_deref(), Some("ollie://blob/44444444-4444-4444-4444-444444444444"));

        // non-blob tools and upload_blob produce no links.
        assert!(blob_resource_links("list_loads", &json!({ "items": [] }), &json!({})).is_empty());
        assert!(blob_resource_links("upload_blob", &json!({ "url": "https://x" }), &json!({})).is_empty());
    }

    #[test]
    fn destructive_ops_require_confirmation() {
        // cancel_trip is always destructive; delete_blob only when force=true.
        assert!(is_destructive_op("cancel_trip", &json!({})));
        // #330 parity deletes + cancel_load are unconditionally destructive.
        for name in [
            "cancel_load", "delete_load", "delete_trip", "delete_driver",
            "delete_truck", "delete_trailer", "delete_facility",
        ] {
            assert!(is_destructive_op(name, &json!({})), "{name} must be destructive");
        }
        assert!(is_destructive_op("delete_blob", &json!({ "force": true })));
        assert!(!is_destructive_op("delete_blob", &json!({ "force": false })));
        assert!(!is_destructive_op("delete_blob", &json!({})));
        // non-destructive tools are never gated.
        assert!(!is_destructive_op("list_loads", &json!({})));
        assert!(!is_destructive_op("update_trip", &json!({ "force": true })));
    }

    #[test]
    fn destructive_decision_only_proceeds_on_explicit_confirm() {
        // Explicit confirmation proceeds (no rejection result).
        assert!(destructive_decision("cancel_trip", Some(true)).is_none());

        // Decline, no-content, and error outcomes all abort with an isError result.
        for outcome in [Some(false), None] {
            let reject = destructive_decision("delete_blob", outcome)
                .unwrap_or_else(|| panic!("outcome {outcome:?} must abort"));
            assert_eq!(reject.is_error, Some(true));
            let text = reject.content[0].as_text().map(|t| t.text.clone()).unwrap_or_default();
            assert!(text.contains("not performed"), "abort message: {text}");
        }
    }
}
