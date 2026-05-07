# Driver UI — Design Spec
**Date:** 2026-05-07
**Version target:** v1.3.0
**Author:** Jim Phillips

---

## Overview

A mobile-first progressive web app (PWA) at `/driver` that lets drivers log in and view their assigned trips — past, current, and upcoming. Read-only for v1.3; driver actions (check-calls, arrive/depart, document uploads) are deferred to a future version.

---

## URL Structure

| Path | Purpose |
|------|---------|
| `/driver` | Login screen (redirects to `/driver/trips` if already authenticated) |
| `/driver/trips` | Main trip list — tabbed view |
| `/driver/trips/:id` | Trip detail — stop timeline |
| `/driver/trips/:id/stops/:seq` | Stop detail |

Static files are served by Axum via `ServeDir` at `/driver`. The service worker scopes to `/driver/` and covers both static assets and API calls under `/driver/api/v1/`.

---

## Frontend

**Stack:** Vanilla JS (ES modules), plain CSS, no bundler, no framework.

**PWA requirements:**
- `manifest.json` — app name, icons, `start_url: "/driver"`, `display: "standalone"`, `theme_color`
- Service worker — caches static assets for offline load; does not cache API responses (trip data must be fresh)
- Mobile-first CSS — minimum tap target 44px, system font stack

**Navigation structure:**
- Top segmented tabs: **Past | Current | Upcoming** — filter the trip list
- Bottom navigation bar: hidden in v1.3, reserved for future app sections (Trips, Workflows, Settlements, Chat)

**Trip list card** fields (all tabs):
- Trip number (`T-YYYY-NNNN`)
- Status badge (color-coded)
- Origin → Destination (first stop name → last stop name)
- Stop count + next stop summary (current tab only)
- Progress bar showing completed stops (current tab only)
- Scheduled start date (upcoming/past tabs)

**Trip detail view** — stop timeline (vertical, scrollable):
- Back button → trip list
- Header: trip number, status badge
- Equipment row: truck unit number, trailer unit number(s)
- Load info row: customer ref (`load.customer_ref`), commodity, weight, load notes
- Stop timeline: each stop as a node — completed (green check), next stop (amber pulsing), future (grey)
- Tapping any stop navigates to stop detail

**Stop detail view:**
- Back button → trip detail
- Stop type label + facility name
- Facility address (street, city, state, zip)
- Schedule block: arrive window (`scheduled_arrive` – `scheduled_arrive_end`), estimated dwell
- Actual times block: `actual_arrive`, `actual_depart` — shown as `—` if not yet recorded
- Notes block: `trip_stop.notes` — dispatcher notes; reference numbers (pickup #, BOL, etc.) live here for v1.3
- Commodity + weight from the linked load (repeated from trip detail for context at stop level)

**Note on reference numbers:** `load.customer_ref` holds the primary reference (e.g. Landstar FB#). Additional reference numbers (BOL, pickup #, delivery #) go in `trip_stop.notes` for v1.3. A structured `reference_numbers: [{label, value}]` array on the load model is a candidate for a future version.

---

## Authentication

**Mechanism:** WebAuthn passkey (primary) + PIN fallback. No SMS, no external services.

**Login identifier:** Phone number. Phone remains optional on `DriverRecord` — a driver without a phone on file simply cannot authenticate. Dispatchers set/update phone via the existing `PUT /api/v1/drivers/:id`.

**Passkey flow:**
1. Driver enters phone number → `POST /driver/api/v1/auth/challenge` returns a WebAuthn challenge
2. Device signs with stored passkey → `POST /driver/api/v1/auth/verify` verifies + issues JWT
3. On a new device with no registered passkey, driver falls through to PIN entry

**PIN fallback flow:**
1. Driver enters phone + PIN → `POST /driver/api/v1/auth/pin` verifies bcrypt hash + issues JWT

**PIN initialization:** A driver with no passkey and no PIN cannot log in. Dispatchers set the initial PIN via a new admin endpoint: `POST /api/v1/drivers/:id/pin` (bearer auth, accepts `{pin: "NNNN"}`). The backend bcrypt-hashes it and stores it in `driver_credentials`. Drivers may change their own PIN post-login in a future version.

**JWT:** Short-lived (8 hours), contains `driver_id` (UUID). All driver API endpoints validate this token via a dedicated middleware — completely separate from the global bearer token used by the admin API.

**Passkey registration:** After PIN login on a new device, the UI prompts the driver to register a passkey for future logins. `POST /driver/api/v1/auth/register-passkey` stores the credential.

**Credential storage:** New `driver_credentials` LanceDB table:

| Field | Type | Notes |
|-------|------|-------|
| `driver_id` | UUID | FK → driver record |
| `pin_hash` | String | bcrypt, nullable — driver may have no PIN |
| `passkey_credentials` | String | JSON array of registered WebAuthn credentials |
| `updated_at` | RFC3339 | Last modified |

---

## Driver API — `/driver/api/v1/`

Separate router mounted under `/driver/api/v1/`. All non-auth endpoints require a valid driver JWT. Responses are purpose-built — not the same shapes as the admin API.

### Auth endpoints (no JWT required)

| Method | Path | Description |
|--------|------|-------------|
| POST | `/driver/api/v1/auth/challenge` | Issue WebAuthn challenge for phone number |
| POST | `/driver/api/v1/auth/verify` | Verify passkey response, issue JWT |
| POST | `/driver/api/v1/auth/pin` | Verify PIN, issue JWT |
| POST | `/driver/api/v1/auth/register-passkey` | Register new passkey (requires JWT) |
| POST | `/driver/api/v1/auth/refresh` | Refresh JWT before expiry |

### Data endpoints (JWT required)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/driver/api/v1/me` | Driver profile: name, phone, status |
| GET | `/driver/api/v1/trips` | Trips for this driver. Query: `?tab=past`, `?tab=current`, or `?tab=upcoming` |
| GET | `/driver/api/v1/trips/:id` | Enriched trip: stops with resolved facility names + load fields |
| GET | `/driver/api/v1/trips/:id/stops/:seq` | Full stop detail |

### Response shapes

**`GET /driver/api/v1/trips`**
```json
{
  "items": [
    {
      "id": "uuid",
      "trip_number": "T-2026-0042",
      "status": "in_transit",
      "origin": "Chicago, IL",
      "destination": "Detroit, MI",
      "stop_count": 3,
      "stops_completed": 1,
      "scheduled_start": "2026-05-07T07:00:00Z",
      "truck_unit": "TRK-1042",
      "trailer_units": ["TRL-8821"]
    }
  ]
}
```

**`GET /driver/api/v1/trips/:id`**
```json
{
  "id": "uuid",
  "trip_number": "T-2026-0042",
  "status": "in_transit",
  "truck_unit": "TRK-1042",
  "trailer_units": ["TRL-8821"],
  "load": {
    "customer_ref": "FB-29381",
    "commodity": "Auto Parts",
    "weight_lbs": 22400,
    "notes": "Handle with care"
  },
  "stops": [
    {
      "sequence": 0,
      "stop_type": "origin",
      "name": "Chicago, IL",
      "address": null,
      "scheduled_arrive": "2026-05-07T07:00:00Z",
      "scheduled_arrive_end": null,
      "actual_arrive": "2026-05-07T07:08:00Z",
      "actual_depart": "2026-05-07T07:42:00Z",
      "expected_dwell_minutes": null,
      "notes": null
    }
  ]
}
```

**`GET /driver/api/v1/trips/:id/stops/:seq`**
```json
{
  "sequence": 1,
  "stop_type": "pickup",
  "facility_name": "Acme Warehouse",
  "address": {
    "street": "1842 Industrial Blvd",
    "city": "Gary",
    "state": "IN",
    "zip": "46401"
  },
  "scheduled_arrive": "2026-05-07T14:00:00Z",
  "scheduled_arrive_end": "2026-05-07T14:30:00Z",
  "actual_arrive": null,
  "actual_depart": null,
  "expected_dwell_minutes": 45,
  "commodity": "Auto Parts",
  "weight_lbs": 22400,
  "notes": "Back dock only. Pickup #: PU-88821. Call 312-555-0182 on arrival."
}
```

---

## Admin API — no changes

`/api/v1/` is untouched. The driver portal's separate namespace prevents any collision with future dispatcher-facing driver endpoints (e.g. `/api/v1/drivers/:id/trips`).

---

## Backend changes summary

| Change | Details |
|--------|---------|
| New LanceDB table | `driver_credentials` — pin_hash, passkey_credentials, updated_at; uses `merge_insert` keyed on `driver_id` |
| New Axum router | `src/api/driver_portal/` — auth + data endpoints |
| New JWT middleware | `src/api/driver_portal/auth.rs` — validates driver JWT |
| New admin endpoint | `POST /api/v1/drivers/:id/pin` — dispatcher sets initial PIN (bearer auth) |
| Static file serving | `ServeDir` at `/driver` → `static/driver/` |
| WebAuthn library | `webauthn-rs` crate |
| No model changes | `DriverRecord.phone` stays optional; no new fields on existing models |

---

## Out of scope for v1.3

- Driver actions: arrive/depart confirmation, check-calls, document uploads
- Push notifications
- Bottom navigation bar (DOM present but hidden)
- Structured reference number fields on loads (`reference_numbers` array)
- Offline trip data caching (service worker caches assets only)
- Multi-language support

---

## Existing issues included in v1.3.0

The following open GitHub issues carry forward into v1.3.0 alongside the driver UI feature:

- **#21** — `resolve_or_create_facility` swallows DB/embed errors
- **#22** — Search result `total` bounded by `limit`
- **#25** — Document query endpoint (`POST /api/v1/blobs/:id/query`)
