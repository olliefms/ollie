pub mod authorization_code;
pub mod blob;
pub mod fleet_user;
pub mod fleet_user_api_key;
pub mod fleet_user_credentials;
pub mod driver;
pub mod driver_credentials;
pub mod event;
pub mod expense;
pub mod facility;
pub mod load;
pub mod maintenance;
pub mod oauth_client;
pub mod pay;
pub mod permission;
pub mod refresh_token;
pub mod terminal;
pub mod trailer;
pub mod trip;
pub mod truck;

pub use authorization_code::*;
pub use blob::*;
pub use fleet_user::*;
pub use fleet_user_api_key::*;
pub use fleet_user_credentials::*;
pub use driver::*;
pub use driver_credentials::*;
pub use event::*;
pub use expense::*;
pub use facility::*;
pub use load::*;
pub use maintenance::*;
pub use oauth_client::*;
pub use pay::{DriverPay, RateSchedule};
pub use permission::*;
pub use refresh_token::*;
pub use terminal::*;
pub use trailer::*;
pub use trip::*;
pub use truck::*;

/// Serde deserializer for `Option<Option<T>>` "double option" fields.
///
/// Pair with `#[serde(default, deserialize_with = "double_option")]`:
/// - absent field  → `None`        ("leave unchanged")
/// - explicit null → `Some(None)`  ("clear")
/// - a value       → `Some(Some)`  ("set")
pub(crate) fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    serde::Deserialize::deserialize(de).map(Some)
}
