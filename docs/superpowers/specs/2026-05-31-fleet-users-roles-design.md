# Fleet Users & Roles — Design

**Date:** 2026-05-31
**Status:** Approved (brainstorm), pending implementation plan
**Issue:** B in the admin-API-removal program (see "Program context"). New epic to be filed.

## Program context

Brainstorming #236 ("deprecate and remove the admin API") revealed it is the tip of a
three-issue program. The admin API cannot be removed until the dispatch surface is
feature-complete **and** dispatcher-account provisioning has a non-admin home.

- **Issue A — Complete dispatch-surface parity.** Mechanical gap-fill so every operational
  admin capability exists on the dispatch surface on **both** HTTP and MCP: `DELETE` for
  driver/trip/load/truck/trailer; customer-facing load `invoice`/`cancel`/`settle`; driver
  `pin` set; and even-up single-surface stragglers (`create_trip` on HTTP;
  `create_driver`/`update_driver` on MCP; facility delete on MCP). Verify the load-stop
  `arrive`/`depart` admin endpoints aren't already covered by the #234 trip→load cascade
  before porting them. Unblocked; can start now.
- **Issue B — Fleet users & roles (THIS SPEC).** The user/role/permission model that replaces
  the admin API's `/api/v1/dispatchers*` provisioning. Unblocks deletion of the last admin
  routes.
- **Issue C — Re-scoped #236 teardown.** Remove `/api/v1/*`, migrate the ~249
  `Bearer test-secret` integration tests, delete `src/api/trip_actions.rs`. Blocked by A + B.

This spec covers **B only**.

## Scope boundary (hard constraint)

This design targets single-fleet scope. Multi-tenancy is intentionally out of scope for
this repository; per-tenant/org concerns are handled outside it.

Therefore everything here is **single-fleet**: users and roles exist within one implicit
fleet. There is **no `fleet` entity, no `fleet_id`, no tenancy plumbing**. The commercial
tenancy is handled outside this repository. "Fleet owner" here means "the root user of this
single-fleet instance," not "tenant."

## Goals

- A user identity with a **role** and optional **per-user permission grants**.
- A **scope-based permission model** (proper ACL) that is the single primitive behind roles,
  per-user grants, and (later) per-API-key scopes (#243).
- An **admin-only Users management surface** (HTTP + MCP) that fully replaces
  `/api/v1/dispatchers*`.
- A **first-run setup wizard** to bootstrap the initial owner (replacing the admin Bearer-key
  provisioning path).
- Reuse the existing dispatcher auth stack with minimal change.

## Non-goals (explicitly out of scope for B)

- Multi-tenancy / `fleet_id` of any kind (out of scope for this repository).
- The `/dispatch`→`/fleet` and `dispatcher`→`user` **rename** — a large mechanical rename with
  zero behavior change; its own issue.
- **Per-API-key scope assignment UI (#243)** — B ships the scope primitive and enforcement and
  the `extra_scopes` field; the key-scope picker and per-user grant editor UI land with #243.
- Unifying drivers into the user model — drivers remain a separate population with their own
  portal/auth (passkey/PIN, `DRIVER_JWT_SECRET`).
- Admin API removal (Issue C) and parity gap-fill (Issue A).

## The permission model

### Scopes are the atomic primitive

Every capability is a `resource:action` string. Wildcards allowed in the action position and as
a global superuser token.

```
loads:read    loads:write    loads:delete    loads:settle    loads:invoice
trips:read    trips:write    trips:delete
drivers:read  drivers:write  drivers:delete
trucks:read   trucks:write   trucks:delete
trailers:read trailers:write trailers:delete
facilities:read facilities:write facilities:delete
terminals:read terminals:write terminals:delete
blobs:read    blobs:write    blobs:delete
events:read
users:read    users:write    users:delete
api_keys:read api_keys:write api_keys:delete
*                              ← superuser (covers everything)
```

- **`write`** = create + update (CRUD collapsed to read/write/delete per the "CRUD + a few
  elevated verbs" decision).
- **Elevated verbs** (distinct because they mark a real role boundary): `loads:settle`,
  `loads:invoice` (customer-facing financial actions, separate from driver-facing trip
  settlement).
- Wildcard matching: a required scope `S = "drivers:write"` is satisfied if the effective set
  contains `drivers:write`, `drivers:*`, or `*`. (`resource:*` and the global `*`.)

### Roles are named scope bundles

| Role | Scopes | Notes |
|---|---|---|
| **owner** | `*` | Superuser. At least one always exists. Cannot be demoted or deleted except via ownership transfer. |
| **fleet_manager** | `*` | Owner-equivalent operationally, incl. `users:*` and `loads:settle/invoice`. Constrained only by **owner-protection rules** below (cannot delete/demote the owner; cannot transfer ownership). |
| **dispatcher** | `loads:read/write`, `trips:read/write/delete`, `drivers:read/write`, `trucks:read/write`, `trailers:read/write`, `facilities:read/write`, `terminals:read`, `blobs:read/write`, `events:read`, `api_keys:read/write/delete` (own keys) | Operational. No `users:*`, no `loads:settle/invoice`, no master-data deletes (drivers/trucks/trailers/facilities/loads). |

Owner and fleet_manager hold the same scope set (`*`); the distinction is enforced by
**owner-protection rules**, not scopes:

- Exactly one owner must always exist.
- The owner record cannot be deleted or have its role changed by anyone (including
  fleet_managers) except through an explicit **ownership transfer**.
- **Ownership transfer** (owner-only): the current owner promotes another user to `owner` and is
  simultaneously demoted to `fleet_manager`, atomically.

The dispatcher matrix above is a sensible default and is tunable during implementation.

### Per-user grants

A user's **effective scopes = role bundle ∪ `extra_scopes`**. This makes one-off elevations
trivial without inventing new roles — e.g. a dispatcher who may also settle loads is
`role=dispatcher, extra_scopes=["loads:settle"]`. Owner stays `*` (grants are a no-op for it).

### Relationship to #243 (per-key scopes)

An API key will carry `scopes ⊆ its user's effective scopes`; per-request authority =
`user_effective_scopes ∩ key_scopes`. Same vocabulary, no new concepts. B builds the
vocabulary, the matcher, and enforcement; #243 adds key-scope storage + the picker UI.

## Data model

Extend the existing dispatcher record (reuse decision — no new table):

```rust
pub struct DispatcherRecord {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub status: DispatcherStatus,
    pub role: Role,                 // NEW: owner | fleet_manager | dispatcher
    pub extra_scopes: Vec<String>,  // NEW: per-user grants beyond the role bundle
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub enum Role { Owner, FleetManager, Dispatcher }
```

LanceDB schema (`dispatcher_schema`): add two columns:

- `role` — `STRING` (snake_case enum string), default `"dispatcher"`.
- `extra_scopes` — JSON-encoded `STRING` array (e.g. `["loads:settle"]`), default `"[]"`.
  Stored as a JSON string column rather than a Lance list to keep ops simple and avoid the
  recurring Arrow/SQL cast gotchas; revisit only if list-native querying is needed.

The credentials table (`DispatcherCredentials`) is unchanged.

## Enforcement

- The dispatcher auth middleware (`src/api/dispatcher_portal/middleware.rs`) already loads the
  user record + credentials on every request (for `token_version`/status/lockout). Extend it to
  read `role` + `extra_scopes`, compute the **effective scope set**, and attach it to the
  request (claims/extension). Role is therefore always **fresh from the DB** — a role/grant
  change takes effect on the next request with no token-version bump or stale-claims problem.
- Provide a small `require_scope("drivers:write")` guard (extractor or per-handler check) used
  by HTTP handlers.
- For MCP tools, perform the same effective-scope check inside tool dispatch.
- Each protected HTTP route and MCP tool declares the single scope it needs. A scope→route/tool
  map is part of the implementation; the matcher handles wildcard expansion.

## Users management surface (replaces `/api/v1/dispatchers*`)

Built on both HTTP and MCP (parity bar), gated by `users:*` scopes (owner + fleet_manager by
default). Mirrors and supersedes the admin endpoints:

HTTP (under the dispatch portal; path settles with the rename issue):

- `POST   .../users`                 → create user (email, name, role, optional extra_scopes) — `users:write`
- `GET    .../users`                 → list users — `users:read`
- `GET    .../users/{id}`            → get user — `users:read`
- `PATCH  .../users/{id}`            → update name/status/role/extra_scopes — `users:write`
- `PUT    .../users/{id}/password`   → reset password (bumps token_version) — `users:write`
- `DELETE .../users/{id}`            → deactivate/remove user — `users:delete`

MCP tools (parity): `list_users`, `get_user`, `create_user`, `update_user`,
`reset_user_password`, `delete_user`.

Owner-protection rules (above) are enforced in these handlers: the owner cannot be
deleted/demoted except via transfer; role assignment to `owner` triggers the transfer flow.

## Owner bootstrap — first-run setup wizard

Replaces the admin Bearer-key provisioning path.

- When the user table is **empty**, the app serves an **unauthenticated** one-time setup page
  and a `POST .../setup` endpoint that creates the first user with `role=owner`.
- The setup endpoint/page is guarded by `user_count == 0`. Once any user exists it returns
  `409/410` and the page redirects to login. No auth is needed precisely because no users exist
  yet (and the guard slams shut the instant one does).

## Migration (existing installs)

- Existing dispatchers migrate to `role=dispatcher`, `extra_scopes=[]`.
- An existing install already has dispatchers, so the empty-table wizard won't fire and no owner
  would exist. Migration rule: **auto-promote the oldest dispatcher** (lowest `created_at`) to
  `role=owner`. Documented in the migration so operators know who holds the root account; the
  owner can then transfer if desired.
- Fresh installs use the wizard; the two paths are mutually exclusive (empty vs non-empty table).

## Testing

- Unit: scope matcher (wildcard expansion, `resource:*`, `*`), role→bundle expansion, effective
  set = bundle ∪ extra_scopes.
- Integration (HTTP): each protected route rejects insufficient scope (403) and accepts
  sufficient; dispatcher cannot reach `users:*` or `loads:settle`; per-user grant elevates a
  dispatcher for exactly the granted scope.
- Owner-protection: cannot delete/demote owner; transfer demotes prior owner atomically;
  at-least-one-owner invariant holds.
- Bootstrap: setup works on empty table, is sealed once a user exists.
- Migration: oldest dispatcher promoted to owner; others become dispatchers.
- MCP parity: each Users tool enforces the same scopes as its HTTP twin.

## Open questions / risks

- **PIN reset granularity:** driver `pin` set currently folds under `drivers:write`. Promote to
  an elevated `drivers:set_pin` if a tighter boundary is wanted. (Default: keep under `write`.)
- **Dispatcher deletes:** proposed dispatcher can `trips:delete` but not `loads:delete` or
  master-data deletes. Confirm during implementation.
- **`extra_scopes` storage:** JSON-string column chosen for simplicity; confirm no querying need
  argues for a native list column.
- **Transfer atomicity:** ownership transfer must be a single atomic write (promote + demote);
  verify the ops layer supports it without a partial-state window.
- **MCP scope plumbing:** confirm the cleanest hook point for per-tool scope checks in
  `dispatcher_portal/mcp.rs`.
