// tests/it.rs — one binary for every integration test that does not mutate
// process-global env. Each former top-level test file is a module here; adding
// a new file means adding a `mod` line, not a new full-dependency link step.
//
// Tests that call std::env::set_var with values that must not leak across
// tests live in tests/isolated.rs instead — see the header there before
// moving anything across.

#[allow(dead_code)]
#[path = "../common/mod.rs"]
mod common;

mod administrative_loads_test;
mod blob_retry_test;
mod divert_test;
mod driver_equipment_test;
mod driver_expenses_test;
mod expenses_test;
mod fleet_pagination_test;
mod fleet_static_cache_test;
mod health_test;
mod integration_test;
mod load_delivery_cascade_test;
mod maintenance_test;
mod migration_test;
mod refresh_token_flow;
mod terminals_pay_settlement_test;
mod tonu_test;
mod trip_lifecycle_fixes_test;
