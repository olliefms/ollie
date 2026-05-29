# MCP OAuth 2.1 + Unified Refresh Tokens — Design

**Date:** 2026-05-28
**Status:** Approved, ready for implementation plan
**Issues:** closes #305; unblocks the Claude Desktop connector path. Independent of #105 (rmcp) and #236 (admin-API removal); designed to align with both.

## Motivation

`POST /dispatch/mcp` authenticates only with short-lived dispatcher JWTs and static `olld_` API keys. **Claude Desktop cannot connect.** Desktop treats a remote `type: "http"` MCP server as an OAuth-protected connector: on any `401` it ignores static `Authorization` headers and runs the MCP authorization flow (OAuth 2.1 authorization-code + PKCE, discovery metadata, Dynamic Client Registration). Ollie exposes no OAuth, so Desktop's registration step fails (`oauth_error=mcp_registration_failed`) and the connector can't be added (#305).

The only Desktop-capable workaround today is bundling a local stdio↔HTTP proxy in the plugin (a Node/Bun runtime dependency we deliberately removed). Adding OAuth lets the plugin point Desktop straight at the production URL.

Separately, the existing JWT refresh is broken for humans: `POST /dispatch/auth/refresh` requires an **unexpired** JWT, so once the 8h access token lapses (e.g. overnight) the refresh call can't authenticate and the user is forced to log in again. The "7-day refresh window" is only reachable while actively refreshing inside each 8h slice. This design fixes that by introducing real refresh tokens, and reuses the same machinery for both the PWA and OAuth.

## Decisions

- **Hand-roll the OAuth Authorization Server in-process.** No Rust crate covers the full surface: `oauth2` is client-only, `rmcp` auth is client-only, and `oxide-auth` lacks DCR (RFC 7591) and discovery metadata (RFC 8414/9728) — exactly the bits Desktop's zero-config connect needs — while being effectively unmaintained. Delegating to an external IdP (e.g. Authentik) was rejected: Authentik has no DCR until a targeted ~2026.8 release, would force a DCR shim + migrating login off bcrypt, and clashes with the in-cab driver PIN UX. Hand-rolling reuses Ollie's existing bcrypt login + JWT issuance, stays self-hosted (no new runtime/identity dependency), and presents Ollie's own login screens.
- **Access token = the existing dispatcher JWT (Approach A).** The OAuth `/token` endpoint mints the same `DispatcherClaims` JWT the portal already issues. The hot path (`require_dispatcher_auth` → `validate_jwt_token`) is unchanged: an OAuth access token simply *is* a dispatcher JWT. Tradeoff: access tokens are not individually revocable before expiry — bounded by the short TTL and the `token_version` global kill switch, which is already how dispatcher JWTs behave.
- **`olld_` API keys stay** as the headless/scripting fallback. OAuth is additive for interactive/Desktop clients. The proposed driver `olldr_` keys are **not** built — the driver MCP (#245), when it lands, uses OAuth.
- **Dispatcher OAuth now; portal-parameterized for driver later.** Build and ship OAuth for `/dispatch/mcp`. The AS is written once, parameterized by a *resource descriptor*; adding the driver resource later is config (a second PRM doc, a `WWW-Authenticate` on the driver MCP route, a driver login/JWT mapping) — not a redesign. Driver MCP tools are out of scope here.
- **Consent screen included.** After login, the authorize flow shows an explicit "Claude Desktop wants to access your Ollie dispatcher account" Allow/Deny step. A remembered-grant record (per client + dispatcher) avoids re-prompting on silent re-auth and is the future home for per-scope consent (#243).
- **Unified refresh-token model across the PWA and OAuth.** Short access token (8h, unchanged, invisible) + long-lived (14d sliding, rotating, hashed-at-rest) refresh token that works even after the access token expires. Applied to dispatcher PWA, driver PWA, and OAuth clients alike. This both fixes the human overnight-logout pain and provides the refresh path Desktop needs.
- **Module placement:** a new portal-agnostic `src/api/oauth/` module (the AS) and a shared refresh-token store. Not under `dispatcher_portal/` because the driver portal will reuse it. This matches the post-#236 two-portal end state (dispatcher + driver are the only long-lived surfaces; the admin API is being removed and is explicitly **not** an OAuth consumer).

## Architecture

Two pieces ship together because they share the new refresh-token machinery:

**A. `src/api/oauth/`** — OAuth 2.1 Authorization Server, parameterized by a `ResourceDescriptor { subject_type, login_flow, jwt_minter, mcp_path }`. One resource registered today: `dispatcher`.

**B. Shared refresh-token store** — used by the OAuth `/token` endpoint and by the dispatcher + driver PWA login/refresh.

**Unchanged:** `require_dispatcher_auth` → `validate_jwt_token` (hot path); the `olld_` API-key branch and `/dispatch/api-keys` endpoints; the hand-rolled `mcp.rs` tool logic. The rmcp migration (#105) remains a separate effort; the only seam to preserve is that OAuth auth + the `WWW-Authenticate`-on-401 stay *scoped to the route* (a `map_response` layer), so re-mounting the route as an rmcp tower service later needs no auth rework.

**Credential types after this lands** (all resolve to `DispatcherClaims` behind the middleware):
1. PWA session — short access JWT + long refresh token (new).
2. OAuth client (Claude Desktop) — short access JWT + long refresh token via the OAuth flow.
3. `olld_` API key — unchanged, headless/scripting.

## Data Model

### New table `refresh_tokens` (shared by PWA + OAuth)

| Column | Type | Notes |
|---|---|---|
| `id` | `uuid` PK | |
| `token_hash` | `text NOT NULL` | SHA-256 hex of the opaque token; lookup path |
| `subject_type` | `text NOT NULL` | `dispatcher` or `driver` |
| `subject_id` | `uuid NOT NULL` | dispatcher_id or driver_id |
| `client_id` | `uuid NULL` FK → `oauth_clients(id)` | NULL = PWA session; set = OAuth client |
| `family_id` | `uuid NOT NULL` | groups a rotation chain (reuse detection) |
| `token_version` | `bigint NOT NULL` | snapshot at issue; refresh rejects on mismatch with current creds |
| `issued_at` | `timestamptz NOT NULL` | |
| `expires_at` | `timestamptz NOT NULL` | `now + 14d`, re-stamped each rotation (sliding) |
| `consumed_at` | `timestamptz NULL` | set when rotated; replay of a consumed token ⇒ theft |
| `revoked_at` | `timestamptz NULL` | |
| `last_used_at` | `timestamptz NULL` | best-effort |

Indexes: unique on `token_hash`; index on `family_id`; index on `(subject_type, subject_id)`.

### New table `oauth_clients` (DCR)

| Column | Type | Notes |
|---|---|---|
| `id` (client_id) | `uuid` PK | returned to the client |
| `client_name` | `text NULL` | from registration metadata |
| `redirect_uris` | `text[] NOT NULL` | exact-match set |
| `created_at` | `timestamptz NOT NULL` | |

Public clients only (PKCE, no secret; `token_endpoint_auth_method: "none"`).

### New table `authorization_codes`

| Column | Type | Notes |
|---|---|---|
| `code_hash` | `text NOT NULL` | SHA-256 hex; one-time |
| `client_id` | `uuid NOT NULL` FK → `oauth_clients(id)` | |
| `redirect_uri` | `text NOT NULL` | must match at token exchange |
| `code_challenge` | `text NOT NULL` | S256 |
| `subject_type` / `subject_id` | | the authenticated dispatcher |
| `resource` | `text NOT NULL` | e.g. `.../dispatch/mcp` |
| `scope` | `text NULL` | single default scope in v1 |
| `expires_at` | `timestamptz NOT NULL` | ~5 min |
| `consumed_at` | `timestamptz NULL` | |

### Optional table `oauth_consent_grants`

`(client_id, subject_type, subject_id, granted_at)` — remembered consent so silent re-auth doesn't re-prompt; revoking the connector deletes the row.

## Token Model & Lifetimes

- **Access token:** existing `DispatcherClaims` JWT (driver analogue later), TTL **8h** (tunable). Validated statelessly; hot path unchanged.
- **Refresh token:** opaque high-entropy string, stored SHA-256-hashed, **14d sliding, rotating**.

**Rotation + theft detection:** each `/refresh` (or OAuth `refresh_token` grant) consumes the presented token, appends a new row in the same `family_id` with a fresh 14d expiry, and returns a new access JWT + new refresh token. Replay of an already-consumed token ⇒ revoke the entire family.

**Kill switch:** the refresh row snapshots `token_version`; refresh rejects on mismatch with current creds. Bumping `token_version` (password change / "log out everywhere") invalidates all access tokens (≤8h lag) and all refresh tokens (next refresh fails), for both PWA and OAuth.

**PWA application (dispatcher + driver):** login (bcrypt/PIN) issues access JWT + refresh token; the refresh token lives in an **HttpOnly, Secure, SameSite** cookie, access token in memory. The front-end silently calls `/refresh` on expiry/401 — and it works **after** the 8h access token is dead, because the refresh token is independent. A new `/logout` revokes the family and clears the cookie. The old "refresh requires unexpired JWT" endpoint is replaced.

## OAuth 2.1 Surface

| Endpoint | Spec | Behavior |
|---|---|---|
| `401` + `WWW-Authenticate` on `/dispatch/mcp` | — | `Bearer resource_metadata="https://…/.well-known/oauth-protected-resource/dispatch/mcp"`. **Scoped to the MCP route only** via a `map_response` layer; REST/portal 401s stay bare (no global `error.rs` change). |
| `GET /.well-known/oauth-protected-resource` (+ path-suffixed `/dispatch/mcp`) | RFC 9728 | `{ resource, authorization_servers: ["https://ollie…"] }` |
| `GET /.well-known/oauth-authorization-server` | RFC 8414 | `issuer`, `authorization_endpoint`, `token_endpoint`, `registration_endpoint`, `response_types_supported:["code"]`, `grant_types_supported:["authorization_code","refresh_token"]`, `code_challenge_methods_supported:["S256"]`, `token_endpoint_auth_methods_supported:["none"]` |
| `POST /oauth/register` | RFC 7591 | DCR. Public client, no secret. Stores `client_name` + `redirect_uris`; returns `client_id`. |
| `GET /oauth/authorize` | OAuth 2.1 + PKCE | validate client/redirect/PKCE → authenticate → consent → issue code |
| `POST /oauth/token` | OAuth 2.1 + PKCE | `authorization_code` (verify `code_verifier`) and `refresh_token` grants → issue the token pair |

**Authorize flow** (`GET /oauth/authorize`):
1. Validate `client_id` registered, `redirect_uri` is an exact match, PKCE `code_challenge` present (mandatory).
2. Authenticate: if no valid AS-session cookie, render the existing dispatcher bcrypt login screen (`resource` selects which login — dispatcher today).
3. Consent screen (Allow/Deny); a remembered-grant row skips re-prompting on silent re-auth.
4. On Allow: mint a one-time `authorization_code` bound to the PKCE challenge + dispatcher + resource → `302` to `redirect_uri?code=…&state=…`.

**Token flow** (`POST /oauth/token`):
- `authorization_code`: look up unconsumed/unexpired code, verify `redirect_uri`+`client_id`, verify `SHA256(code_verifier)==code_challenge`, consume → issue access JWT (current `token_version`, 8h) + refresh token (`client_id` set).
- `refresh_token`: rotate per the token model.

**Coexistence on the MCP route:** `Bearer olld_…` (headless) and `Bearer <dispatcher-JWT>` (OAuth-issued) both pass the unchanged `validate_*` logic. No credential ⇒ `401` + `WWW-Authenticate` ⇒ Desktop runs the OAuth dance.

**Driver later = config, not redesign:** add a PRM doc at `/.well-known/oauth-protected-resource/driver/api/v1/mcp`, the `WWW-Authenticate` on the driver MCP route, and a `resource=driver` descriptor (PIN login + driver-claims JWT). `register`/`authorize`/`token` and the AS-metadata doc serve both.

## Error Handling

A small `OauthError` type local to `oauth/` (keeps global `error.rs` clean):
- **Authorize errors:** if `redirect_uri` is valid, redirect with `?error=…&state=…` per RFC 6749 (`access_denied` on Deny, `invalid_request`, `unsupported_response_type`, …). If `client_id`/`redirect_uri` itself is invalid → **do not redirect** (open-redirect risk); render a plain error page.
- **Token / DCR errors:** JSON `{error, error_description}` with correct codes (`invalid_grant`, `invalid_client`, `invalid_request`, `unsupported_grant_type`; `invalid_redirect_uri`/`invalid_client_metadata` for DCR).

## Security Hardening

- PKCE **S256 mandatory** — reject missing/`plain`.
- `redirect_uri` **exact-match** against the registered set at authorize (no open redirect). DCR accepts loopback (`http://127.0.0.1[:port]`) and Desktop's custom scheme; rejects arbitrary hosts.
- Authorization codes: ~5 min TTL, one-time, bound to `client_id`+`redirect_uri`+PKCE+subject.
- Refresh tokens: hashed at rest; rotation + reuse-detection family-revoke; 14d sliding; PWA refresh in HttpOnly/Secure/SameSite cookie.
- `state` passthrough (client CSRF); short-lived HttpOnly AS-session cookie for the authorize login.
- Reuse the existing bcrypt lockout (`locked_until`) on the authorize login.
- `token_version` global kill switch.
- Never log tokens/codes/verifiers; constant-time hash compares.

## Testing

- **Unit:** PKCE verify; code issue/consume; refresh rotation + reuse→family-revoke; `token_version` kill; metadata doc contents; `redirect_uri` matching.
- **Integration (`axum-test`):** full DCR → authorize (login + consent) → token (code+PKCE) → call `/dispatch/mcp` with the JWT; refresh rotation; reuse→`401`+revoke; `401` on `/dispatch/mcp` carries `WWW-Authenticate` while a REST `401` does not; metadata JSON correct; `olld_` still works; PWA login sets the refresh cookie and silently refreshes **after** the 8h access token expires (regression test for the overnight-logout fix). Negatives: missing PKCE, bad verifier, expired/reused code, `redirect_uri` mismatch, expired/revoked refresh.
- **Reusable helper** that mints a dispatcher JWT via the flow — reusable by #236's test migration.
- **Manual acceptance (the real #305 test):** add Ollie as a connector in Claude Desktop and confirm end-to-end connection.

## Out of Scope (YAGNI)

- Client secrets / confidential clients.
- Scopes beyond a single default scope (#243 per-scope deferred; the consent screen is its future home).
- External IdP delegation.
- Any rmcp / #105 change; MCP tool logic.
- Driver MCP tools (#245) — only the parameterization seam ships.
- Per repo convention the codebase is hand-formatted — do not run `cargo fmt --all`.

## Sequencing

Auth (this spec) ships first — it unblocks the filed Desktop blocker (#305) and is independent of the rmcp migration (#105), which follows to deliver protocol compliance + the #292–#301 feature bundle. The `WWW-Authenticate` scoping keeps the rmcp re-mount auth-rework-free.
