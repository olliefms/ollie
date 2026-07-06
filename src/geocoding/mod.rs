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

#[derive(Deserialize)]
struct NominatimMatch {
    lat: String,
    lon: String,
    display_name: String,
}

impl Default for GeocodingClient {
    fn default() -> Self {
        // A descriptive User-Agent is required by the Nominatim usage policy and
        // is harmless for Census. Fall back to a bare client if the builder fails.
        let client = Client::builder()
            .user_agent(concat!("ollie-tms/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }
}

impl GeocodingClient {
    pub fn new() -> Self {
        Self::default()
    }

    /// If the address string looks like "lat,lng", parse it directly.
    /// Otherwise call the Census Bureau geocoder, falling back to OpenStreetMap's
    /// Nominatim when Census can't resolve the address (its address-range data
    /// misses many valid new/industrial-park addresses).
    /// Returns (lat, lng, display_address).
    pub async fn geocode(&self, address: &str) -> Option<(f64, f64, String)> {
        if let Some((lat, lng)) = self.parse_lat_lng(address) {
            return Some((lat, lng, address.trim().to_string()));
        }
        if let Some(hit) = self.geocode_via_census(address).await {
            return Some(hit);
        }
        self.geocode_via_nominatim(address).await
    }

    /// Parses "lat,lng" or "lat, lng" strings. Returns None for anything else.
    pub fn parse_lat_lng(&self, s: &str) -> Option<(f64, f64)> {
        let parts: Vec<&str> = s.splitn(2, ',').collect();
        if parts.len() != 2 { return None; }
        let lat = parts[0].trim().parse::<f64>().ok()?;
        let lng = parts[1].trim().parse::<f64>().ok()?;
        if !lat.is_finite() || !lng.is_finite() { return None; }
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

    /// OpenStreetMap Nominatim fallback. Returns the top match's (lat, lng,
    /// display_name). Any network/parse failure yields None, same as Census.
    async fn geocode_via_nominatim(&self, address: &str) -> Option<(f64, f64, String)> {
        let resp = self.client
            .get("https://nominatim.openstreetmap.org/search")
            .query(&[("q", address), ("format", "json"), ("limit", "1")])
            .timeout(std::time::Duration::from_secs(10))
            .send().await.ok()?;

        let data: Vec<NominatimMatch> = resp.json().await.ok()?;
        let m = data.into_iter().next()?;
        let lat = m.lat.parse::<f64>().ok()?;
        let lng = m.lon.parse::<f64>().ok()?;
        if !lat.is_finite() || !lng.is_finite() || lat.abs() > 90.0 || lng.abs() > 180.0 {
            return None;
        }
        Some((lat, lng, m.display_name))
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

    #[tokio::test]
    #[ignore] // requires network: cargo test geocoding -- --ignored
    async fn test_geocode_nominatim_fallback_live() {
        // An address Census's range data typically misses but OSM resolves.
        let client = GeocodingClient::new();
        let result = client
            .geocode_via_nominatim("2285 Valentine Industrial Pkwy, Jefferson, GA 30549")
            .await;
        assert!(result.is_some());
        let (lat, lng, addr) = result.unwrap();
        assert!((lat - 34.169).abs() < 0.1);
        assert!((lng - (-83.636)).abs() < 0.1);
        assert!(!addr.is_empty());
    }
}
