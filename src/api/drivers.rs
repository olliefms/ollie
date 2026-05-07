// TODO: filled in by issue #28 (Drivers CRUD + state machine)
use crate::AppState;
use axum::Router;

pub fn router() -> Router<AppState> {
    Router::new()
}
