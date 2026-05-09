// src/api/dispatcher_portal/jwt.rs
use crate::error::AppError;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const ISSUER: &str = "ollie-dispatcher";
const AUDIENCE: &str = "ollie-dispatcher";
const KID: &str = "v1";
const EXPIRY_SECS: u64 = 8 * 3600;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DispatcherClaims {
    pub dispatcher_id: String,
    pub token_version: i64,
    pub iss: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
    pub kid: String,
}

pub fn encode_dispatcher_jwt(id: Uuid, token_version: i64, secret: &str) -> Result<String, AppError> {
    let now = jsonwebtoken::get_current_timestamp() as usize;
    let claims = DispatcherClaims {
        dispatcher_id: id.to_string(),
        token_version,
        iss: ISSUER.into(),
        aud: AUDIENCE.into(),
        exp: now + EXPIRY_SECS as usize,
        iat: now,
        kid: KID.into(),
    };
    let header = Header { kid: Some(KID.into()), ..Header::default() };
    encode(&header, &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .map_err(|e| AppError::Internal(format!("jwt encode error: {e}")))
}

pub fn decode_dispatcher_jwt(token: &str, secret: &str) -> Result<DispatcherClaims, AppError> {
    let mut validation = Validation::default();
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[AUDIENCE]);
    decode::<DispatcherClaims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation)
        .map(|data| data.claims)
        .map_err(|_| AppError::Unauthorized)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret-that-is-long-enough-for-hs256";

    #[test]
    fn test_encode_decode_roundtrip() {
        let dispatcher_id = Uuid::new_v4();
        let token_version = 42i64;
        let token = encode_dispatcher_jwt(dispatcher_id, token_version, SECRET).unwrap();
        let claims = decode_dispatcher_jwt(&token, SECRET).unwrap();
        assert_eq!(claims.dispatcher_id, dispatcher_id.to_string());
        assert_eq!(claims.token_version, token_version);
        assert_eq!(claims.iss, ISSUER);
        assert_eq!(claims.aud, AUDIENCE);
        assert_eq!(claims.kid, KID);
    }

    #[test]
    fn test_wrong_secret_rejected() {
        let dispatcher_id = Uuid::new_v4();
        let token = encode_dispatcher_jwt(dispatcher_id, 1, SECRET).unwrap();
        let result = decode_dispatcher_jwt(&token, "wrong-secret-that-is-also-long-enough");
        assert!(matches!(result, Err(AppError::Unauthorized)));
    }

    #[test]
    fn test_expired_token_rejected() {
        use jsonwebtoken::{EncodingKey, Header, encode};
        let dispatcher_id = Uuid::new_v4();
        // Build a token with exp in the past
        let claims = DispatcherClaims {
            dispatcher_id: dispatcher_id.to_string(),
            token_version: 1,
            iss: ISSUER.into(),
            aud: AUDIENCE.into(),
            exp: 1_000_000, // far in the past
            iat: 999_999,
            kid: KID.into(),
        };
        let token = encode(&Header::default(), &claims, &EncodingKey::from_secret(SECRET.as_bytes())).unwrap();
        let result = decode_dispatcher_jwt(&token, SECRET);
        assert!(matches!(result, Err(AppError::Unauthorized)));
    }
}
