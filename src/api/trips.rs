// TODO: filled in by issue #31 (Trips entity + stops + load cascade)
use crate::AppState;
use axum::Router;

pub fn router() -> Router<AppState> {
    Router::new()
}
