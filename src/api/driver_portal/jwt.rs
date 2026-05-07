// src/api/driver_portal/jwt.rs
use crate::error::AppError;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const ISSUER: &str = "ollie-driver";
const AUDIENCE: &str = "ollie-driver";
const KID: &str = "v1";
const EXPIRY_SECS: u64 = 8 * 3600;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DriverClaims {
    pub driver_id: String,
    pub token_version: i64,
    pub iss: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
    pub kid: String,
}

pub fn encode_driver_jwt(driver_id: Uuid, token_version: i64, secret: &str) -> Result<String, AppError> {
    let now = jsonwebtoken::get_current_timestamp() as usize;
    let claims = DriverClaims {
        driver_id: driver_id.to_string(),
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

pub fn decode_driver_jwt(token: &str, secret: &str) -> Result<DriverClaims, AppError> {
    let mut validation = Validation::default();
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[AUDIENCE]);
    decode::<DriverClaims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation)
        .map(|data| data.claims)
        .map_err(|_| AppError::Unauthorized)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret-that-is-long-enough-for-hs256";

    #[test]
    fn test_encode_decode_roundtrip() {
        let driver_id = Uuid::new_v4();
        let token_version = 42i64;
        let token = encode_driver_jwt(driver_id, token_version, SECRET).unwrap();
        let claims = decode_driver_jwt(&token, SECRET).unwrap();
        assert_eq!(claims.driver_id, driver_id.to_string());
        assert_eq!(claims.token_version, token_version);
        assert_eq!(claims.iss, ISSUER);
        assert_eq!(claims.aud, AUDIENCE);
        assert_eq!(claims.kid, KID);
    }

    #[test]
    fn test_wrong_secret_rejected() {
        let driver_id = Uuid::new_v4();
        let token = encode_driver_jwt(driver_id, 1, SECRET).unwrap();
        let result = decode_driver_jwt(&token, "wrong-secret-that-is-also-long-enough");
        assert!(matches!(result, Err(AppError::Unauthorized)));
    }

    #[test]
    fn test_expired_token_rejected() {
        use jsonwebtoken::{EncodingKey, Header, encode};
        let driver_id = Uuid::new_v4();
        // Build a token with exp in the past
        let claims = DriverClaims {
            driver_id: driver_id.to_string(),
            token_version: 1,
            iss: ISSUER.into(),
            aud: AUDIENCE.into(),
            exp: 1_000_000, // far in the past
            iat: 999_999,
            kid: KID.into(),
        };
        let token = encode(&Header::default(), &claims, &EncodingKey::from_secret(SECRET.as_bytes())).unwrap();
        let result = decode_driver_jwt(&token, SECRET);
        assert!(matches!(result, Err(AppError::Unauthorized)));
    }
}
