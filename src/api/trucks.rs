// src/api/trucks.rs
//
// Intentionally empty: `ListTrucksQuery` used to live here as dead code (never
// referenced by any handler and absent from the OpenAPI schema registry), while
// the query struct actually wired to GET /fleet/api/v1/trucks lives next to its
// handler in src/api/fleet_portal/data.rs. Removed to stop misleading the docs.
// See src/api/fleet_portal/data.rs::ListTrucksQuery.
