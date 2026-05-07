// TODO: filled in by issue #30 (Trailers CRUD + state machine)
use crate::AppState;
use axum::Router;

pub fn router() -> Router<AppState> {
    Router::new()
}
