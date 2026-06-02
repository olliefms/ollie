# Dispatcher API Keys — Design

**Date:** 2026-05-20
**Status:** Approved, ready for implementation plan

## Motivation

Ollie's `/dispatch/mcp` endpoint sits behind a short-lived JWT obtained via `POST /dispatch/auth/login` and refreshed via `POST /dispatch/auth/refresh`. Claude's remote MCP connector (and any similar server-to-server integration) can only attach a static `Authorization` header — it has no way to log in, refresh, or rotate credentials.

Today we work around this with a local bun-based proxy on the user's Mac that handles the auth lifecycle and re-exposes the MCP tools as a local stdio MCP server. This adds friction (runtime dependencies, a long-lived local process) and — critically — Claude's artifact sandbox can't reach a process bound to localhost, blocking live-refreshing dashboards and scheduled tasks.

The fix is a long-lived bearer credential that can be passed via a static `Authorization: Bearer …` header. With it, `.mcp.json` becomes a single connector entry pointing at the production URL; the proxy disappears.

## Decisions

- **Identity model:** keys are bound to a dispatcher but carry a user-supplied label. They act with the dispatcher's full permissions; the label surfaces in tracing for audit purposes ("hybrid" model).
- **Expiry:** 1-year default, configurable down at creation. Max 365 days in v1. "Never expire" is gated on the management UI existing.
- **Multiplicity:** unlimited keys per dispatcher (soft-capped at 20 active), each with its own label, so revoking one integration doesn't break others.
- **Key format:** `olld_` prefix + 32 bytes of CSPRNG, base62-encoded (~48 chars total). Prefix is greppable by secret scanners.
- **Storage:** SHA-256 hash of the plaintext in the DB. Plaintext is returned exactly once in the creation response and never again.
- **Auth integration:** single middleware (`require_dispatcher_auth`) that branches on bearer prefix — JWT path unchanged, API-key path added. Downstream handlers (MCP, data, blobs) are unmodified.

## Data Model

New table `dispatcher_api_keys`:

| Column | Type | Notes |
|---|---|---|
| `id` | `uuid` PK | |
| `dispatcher_id` | `uuid` FK → `dispatchers(id)` ON DELETE CASCADE | |
| `label` | `text NOT NULL` | user-supplied, 1–64 chars |
| `key_hash` | `text NOT NULL` | SHA-256 hex of the plaintext key |
| `key_prefix` | `text NOT NULL` | first 12 chars of plaintext (e.g. `olld_a1b2c3`) — safe to display |
| `created_at` | `timestamptz NOT NULL` | |
| `expires_at` | `timestamptz NOT NULL` | default `created_at + 1 year` |
| `revoked_at` | `timestamptz NULL` | soft-delete; non-null = revoked |
| `last_used_at` | `timestamptz NULL` | best-effort, async updates |

Indexes:
- Unique on `key_hash` (auth lookup path).
- Secondary on `(dispatcher_id, revoked_at)` for listing active keys.

## Key Format

`olld_` + base62(32 random bytes from `OsRng`) → ~48 chars total.

- Plaintext returned once in the `POST /dispatch/api-keys` 201 response.
- `key_prefix` = first 12 chars (`olld_` + first 7 base62 chars). Stored and surfaced everywhere a key is referenced after creation.
- SHA-256 of the plaintext is the storage form. Argon2/bcrypt is unnecessary: 32 bytes from a CSPRNG makes brute-force computationally implausible, and SHA-256 lets the auth middleware verify on every request cheaply.

## Endpoints

All three sit under `/dispatch/api-keys`, behind `require_dispatcher_auth`. **API keys themselves cannot create or revoke other API keys in v1** — those operations require a JWT, to prevent a leaked key from being self-renewing.

### `POST /dispatch/api-keys` — create

Request:
```json
{ "label": "Claude desktop", "expires_in_days": 365 }
```

- `label`: required, non-empty, ≤64 chars.
- `expires_in_days`: optional, default 365, range [1, 365].

Response `201`:
```json
{
  "id": "uuid",
  "label": "Claude desktop",
  "key": "olld_...",
  "key_prefix": "olld_a1b2c3",
  "created_at": "...",
  "expires_at": "..."
}
```

Error responses:
- `400` — invalid label or `expires_in_days` out of range.
- `401` — JWT missing/invalid (or request authed via API key, which is not allowed for this endpoint).
- `429` — dispatcher already has 20 active keys.

### `GET /dispatch/api-keys` — list

Response `200`:
```json
{
  "keys": [
    {
      "id": "uuid",
      "label": "Claude desktop",
      "key_prefix": "olld_a1b2c3",
      "created_at": "...",
      "expires_at": "...",
      "last_used_at": "..."
    }
  ]
}
```

Returns only the calling dispatcher's keys, excluding revoked ones.

### `DELETE /dispatch/api-keys/{id}` — revoke

- `204` on success. Sets `revoked_at = now()`.
- `404` if the key doesn't exist *or* belongs to another dispatcher (avoid leaking existence).
- `401` if request is authed via API key.

### OpenAPI

All three handlers use `#[utoipa::path(...)]` annotations matching the existing `dispatcher_portal/auth.rs` convention, tagged `dispatch-api-keys`.

## Auth Middleware

Rename `require_dispatcher_jwt` → `require_dispatcher_auth`. Logic:

```text
extract bearer token from Authorization header
if token starts with "olld_":  validate_api_key(token)
else:                          validate_jwt(token)         # existing path, unchanged
```

### API-key validation path

1. Compute `sha256_hex(token)`.
2. `SELECT * FROM dispatcher_api_keys WHERE key_hash = ?` (uses the unique index).
3. Reject with `401` if: not found, `revoked_at IS NOT NULL`, or `expires_at <= now()`.
4. Load the dispatcher; reject with `401` if `status = Inactive` (mirrors existing JWT logic).
5. Reject with `401` if `dispatcher_credentials.locked_until > now()` (mirrors JWT logic).
6. Fire-and-forget `tokio::spawn` to update `last_used_at = now()`. Errors logged at `warn`, never propagated.
7. Inject extended `DispatcherClaims` into request extensions and pass through.

### Extended `DispatcherClaims`

```rust
pub struct DispatcherClaims {
    pub dispatcher_id: String,
    pub token_version: i32,
    pub iat: usize,
    pub api_key_id: Option<Uuid>,     // None on JWT auth
    pub api_key_label: Option<String>, // None on JWT auth
}
```

JWT path leaves the new fields `None`. API-key path populates them; `iat` is set to `0` and `token_version` to the dispatcher's current value (kept consistent for any code that reads it).

### Tracing

Every authenticated request span records:

- `auth.kind`: `"jwt"` or `"api_key"`.
- `auth.key_id` and `auth.key_label` (API-key path only).

This satisfies the audit-trail requirement without a separate logging system.

### Authorization endpoint policy

`POST /dispatch/api-keys` and `DELETE /dispatch/api-keys/{id}` additionally check `claims.api_key_id.is_none()` and return `401` if set. `GET /dispatch/api-keys` is allowed for both auth methods (read-only and useful for self-inspection from a script).

## Security

- Plaintext keys appear only in (a) the 201 response body of `POST /dispatch/api-keys`, and (b) inbound `Authorization` headers. Never logged, never echoed, never re-displayable.
- Tracing layer redacts `Authorization` headers (verify existing behavior; add if absent).
- Revocation is immediate — no in-memory caching of `key_hash → dispatcher_id` in v1.
- Cascading deactivation: setting `dispatcher.status = Inactive` instantly disables all that dispatcher's keys via the existing dispatcher check in the middleware. Deleting a dispatcher row cascades via the FK.
- Bumping `dispatcher_credentials.token_version` (the "revoke all JWT sessions" lever) does **not** affect API keys. They are an explicitly out-of-band credential; revoke individually via `DELETE /dispatch/api-keys/{id}`.
- Soft cap of 20 active keys per dispatcher to prevent runaway scripts. Returns `429` on the 21st.

## Testing

**Unit:**
- Key generation produces unique 48-char `olld_`-prefixed strings (1000 iterations, no collisions).
- `key_prefix` extraction is the first 12 chars of the plaintext.
- SHA-256 hash is stable across calls.

**Integration** (uses existing `test_state` helper with the in-test database):
- Create → response includes plaintext exactly once; DB row contains only the hash, not the plaintext.
- Create with `expires_in_days = 7` → expiry is `created_at + 7d` (tolerance ±1s).
- Create with `expires_in_days = 366` → `400`.
- List → returns only the calling dispatcher's keys; revoked excluded.
- Revoke → subsequent auth with that key returns `401`.
- Revoke someone else's key → `404`.
- Auth with expired key → `401`.
- Auth with key whose dispatcher is `Inactive` → `401`.
- Auth with valid JWT still works (regression).
- MCP endpoint (`POST /dispatch/mcp`) reachable with `olld_` bearer and returns a valid `initialize` response.
- 21st active key creation → `429`.
- `POST /dispatch/api-keys` authed via API key → `401`.

**End-to-end smoke:** `curl -H "Authorization: Bearer olld_…" https://…/dispatch/mcp` returns a valid MCP `initialize` response.

## MCP Wiring

The MCP endpoint already sits behind `require_dispatcher_jwt`. After the rename to `require_dispatcher_auth`, the MCP route is unchanged and the connector config becomes:

```json
{
  "mcpServers": {
    "ollie-dispatch": {
      "type": "sse",
      "url": "https://your-ollie-instance.example.com/dispatch/mcp",
      "headers": {
        "Authorization": "Bearer olld_…"
      }
    }
  }
}
```

The local bun proxy can be retired.

## Out of Scope (v1)

These items are deliberately deferred. The "follow-up" column indicates whether a GitHub issue should be filed.

| Item | Rationale | Follow-up issue? |
|---|---|---|
| Management UI in dispatch portal | Endpoints unblock the MCP use case immediately; UI is pure ergonomics. | Yes |
| "Never expire" keys | Gated on a management UI existing, so emergency revocation is reachable. | Yes |
| Per-key scopes/permissions | v1 keys inherit full dispatcher permissions. No demand signal yet. | Yes (`backlog`) |
| IP allowlist per key | Nice-to-have; defer until requested. | No (file on demand) |
| API keys creating/revoking other keys | Design decision, not a deferred feature. | No |
| In-memory cache for `key_hash → dispatcher_id` | Premature optimization; address if perf bites. | No |

## Follow-up Issues

To be filed alongside the implementation PR (titles to confirm at issue-creation time):

- "Dispatcher API key management UI"
- "Allow never-expire dispatcher API keys (requires management UI)"
- "Per-key scopes for dispatcher API keys" (`backlog`)
