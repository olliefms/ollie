# Spec-compliant notification handling for `/dispatch/mcp`

**Date:** 2026-05-28
**Issue:** #105 (partial — spec-compliance slice only)
**Status:** Approved, ready for implementation plan

## Background

The dispatcher MCP server (`POST /dispatch/mcp`) is a hand-rolled JSON-RPC 2.0
implementation in `src/api/dispatcher_portal/mcp.rs`. It works for custom
clients but is not Streamable-HTTP spec-compliant enough for a stock
`type: "http"` MCP client (e.g. Claude Desktop) to connect directly.

Field notes on issue #105 (ergofobe, while building `ollie-claude-plugin`)
identified three observations against a live probe:

1. `initialize` → `200` JSON. Fine.
2. `notifications/initialized` → **`200` + JSON-RPC error** (`-32601 method not
   found`). **Non-compliant.** A notification-only POST MUST return `202
   Accepted` with no body.
3. `GET /dispatch/mcp` → `405`. Spec-acceptable (server offers no stream);
   clients tolerate it.

Only gap #2 is a hard blocker. Because of it, the downstream plugin cannot point
Claude's `type: "http"` transport at the endpoint and instead ships a local
stdio↔HTTP proxy. Fixing #2 lets the plugin drop the proxy and the Node/Bun
runtime entirely.

## Scope decision

This spec covers **only** the spec-compliance fix. The larger axum 0.7 → 0.8
upgrade and rmcp `StreamableHttpService` adoption (the rest of #105) are
deliberately deferred:

- **No security driver.** A RustSec advisory-db scan of the locked dependency
  tree (axum 0.7.9, axum-core 0.4.5, tower-http 0.5.2/0.6.10, hyper 1.9.0,
  matchit 0.7.3) found zero advisories affecting any of them. Dependabot is
  correct.
- **High cost, no incremental payoff for this use case.** The full migration
  requires rewriting 58 `:param` routes to `{param}` across 10 files, bumping
  axum-extra/tower-http/axum-test, and reimplementing 45 tools in rmcp's typed
  `#[tool]` macro system — touching the entire HTTP layer. The user-visible
  payoff for the plugin is identical to the minimal fix.

The rmcp-capability discussion (server→client streaming, resources, prompts) is
tracked separately and is not a blocker here. Issue #105 stays open for the
eventual migration.

## The change

Single root cause: in JSON-RPC 2.0 a message with no `id` is a *notification*.
The server must never reply with a Response to one, and per Streamable HTTP a
notification-only POST must return `202 Accepted` with an empty body. Today
`handle` always builds a `JsonRpcResponse` and returns `200`.

All changes are in `src/api/dispatcher_portal/mcp.rs`:

- Change `handle`'s return type from `Json<JsonRpcResponse>` to
  `axum::response::Response` (via `IntoResponse`).
- Branch on `req.id` at the top of `handle`:
  - **`None` (notification)** → return `StatusCode::ACCEPTED` with an empty
    body. No method dispatch, no error, ever. (Stateless server has no
    notifications with side effects, so accept-and-drop is correct.)
  - **`Some` (request)** → unchanged logic: dispatch `initialize` /
    `tools/list` / `tools/call`, return `200` + `Json(JsonRpcResponse)`,
    including the `-32601` error for genuinely unknown *requests*.
- The `jsonrpc != "2.0"` guard only emits an error Response when an `id` is
  present; a malformed notification still just `202`s (nothing to respond to).

## Explicitly NOT changing

- **GET → 405:** no GET route is registered, so axum already returns a clean
  `405`. Spec-acceptable. Leave it.
- **`protocolVersion: "2024-11-05"`:** `initialize` works as observed; clients
  negotiate down. No change.
- **Sessions / `Mcp-Session-Id`:** stays stateless. No change.
- **Auth, dependencies, axum version, route syntax:** untouched.

## Tests

Added to the existing `#[cfg(test)]` module in `mcp.rs`, using `axum-test`
(same harness as `middleware.rs` tests):

1. `notifications/initialized` (no `id`) → `202`, empty body. *(Direct
   regression for the reported bug.)*
2. Arbitrary notification (no `id`, unknown method) → `202`, empty body.
3. `initialize` (with `id`) → `200`, JSON result present.
4. `tools/list` (with `id`) → `200`, tools array present.
5. Unknown method *with* `id` → `200`, JSON-RPC error `-32601`.

## Risk

Near-zero. One function's signature and top-level branching change; tool
dispatch is untouched. No dependency or route changes — blast radius is the MCP
endpoint only.
