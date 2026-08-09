// tests/isolated.rs — tests that mutate process-global env in ways that must
// not leak into the rest of the suite.
//
// They cannot join tests/it.rs: oauth_flow sets OLLIE_PUBLIC_BASE_URL, which
// every later Config::from_env in the same process would then inherit (turning
// on cookie_secure and presigned URLs suite-wide), and the two pipeline tests
// demand contradictory values of OLLIE_TESSERACT_BIN. Within this binary the
// pipeline pair serialises on common::ENV_LOCK, and startup_recovery_test
// mutates PORT, which every Config::from_env in the main suite reads.

#[allow(dead_code)]
#[path = "../common/mod.rs"]
mod common;

mod oauth_flow;
mod pipeline_empty_vision_test;
mod pipeline_ocr_test;
mod startup_recovery_test;
