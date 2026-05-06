use reqwest::Client;
use serde::Deserialize;

pub struct GeocodingClient {
    client: Client,
}

#[derive(Deserialize)]
struct CensusResponse {
    result: CensusResult,
}

#[derive(Deserialize)]
struct CensusResult {
    #[serde(rename = "addressMatches")]
    address_matches: Vec<AddressMatch>,
}

#[derive(Deserialize)]
struct AddressMatch {
    #[serde(rename = "matchedAddress")]
    matched_address: String,
    coordinates: Coordinates,
}

#[derive(Deserialize)]
struct Coordinates {
    x: f64, // longitude
    y: f64, // latitude
}

impl GeocodingClient {
    pub fn new() -> Self {
        Self { client: Client::new() }
    }

    /// If the address string looks like "lat,lng", parse it directly.
    /// Otherwise call the Census Bureau geocoding API.
    /// Returns (lat, lng, display_address).
    pub async fn geocode(&self, address: &str) -> Option<(f64, f64, String)> {
        if let Some((lat, lng)) = self.parse_lat_lng(address) {
            return Some((lat, lng, address.trim().to_string()));
        }
        self.geocode_via_census(address).await
    }

    /// Parses "lat,lng" or "lat, lng" strings. Returns None for anything else.
    pub fn parse_lat_lng(&self, s: &str) -> Option<(f64, f64)> {
        let parts: Vec<&str> = s.splitn(2, ',').collect();
        if parts.len() != 2 { return None; }
        let lat = parts[0].trim().parse::<f64>().ok()?;
        let lng = parts[1].trim().parse::<f64>().ok()?;
        if lat.abs() > 90.0 || lng.abs() > 180.0 { return None; }
        Some((lat, lng))
    }

    async fn geocode_via_census(&self, address: &str) -> Option<(f64, f64, String)> {
        let resp = self.client
            .get("https://geocoding.geo.census.gov/geocoder/locations/onelineaddress")
            .query(&[("address", address), ("benchmark", "2020"), ("format", "json")])
            .timeout(std::time::Duration::from_secs(10))
            .send().await.ok()?;

        let data: CensusResponse = resp.json().await.ok()?;
        let m = data.result.address_matches.into_iter().next()?;
        Some((m.coordinates.y, m.coordinates.x, m.matched_address))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lat_lng_string() {
        let client = GeocodingClient::new();
        let result = client.parse_lat_lng("35.1495,-90.0490");
        assert!(result.is_some());
        let (lat, lng) = result.unwrap();
        assert!((lat - 35.1495).abs() < 1e-4);
        assert!((lng - (-90.0490)).abs() < 1e-4);
    }

    #[test]
    fn test_parse_lat_lng_string_with_spaces() {
        let client = GeocodingClient::new();
        assert!(client.parse_lat_lng("35.1495, -90.0490").is_some());
    }

    #[test]
    fn test_parse_lat_lng_rejects_plain_address() {
        let client = GeocodingClient::new();
        assert!(client.parse_lat_lng("Memphis, TN").is_none());
    }

    #[tokio::test]
    #[ignore] // requires network: cargo test geocoding -- --ignored
    async fn test_geocode_live() {
        let client = GeocodingClient::new();
        let result = client.geocode("1600 Pennsylvania Ave NW, Washington, DC").await;
        assert!(result.is_some());
        let (lat, lng, addr) = result.unwrap();
        assert!((lat - 38.8977).abs() < 0.01);
        assert!((lng - (-77.0366)).abs() < 0.01);
        assert!(!addr.is_empty());
    }
}
