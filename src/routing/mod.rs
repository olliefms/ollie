// src/routing/mod.rs
use reqwest::Client;
use serde::{Deserialize, Serialize};

pub struct RoutingClient {
    client: Client,
    api_key: String,
}

#[derive(Serialize)]
struct OrsRequest<'a> {
    coordinates: Vec<[f64; 2]>,
    units: &'a str,
}

#[derive(Deserialize)]
struct OrsResponse {
    routes: Vec<OrsRoute>,
}

#[derive(Deserialize)]
struct OrsRoute {
    summary: OrsSummary,
}

#[derive(Deserialize)]
struct OrsSummary {
    distance: f64,
}

impl RoutingClient {
    pub fn new(api_key: &str) -> Self {
        Self { client: Client::new(), api_key: api_key.to_string() }
    }

    /// Calculates total HGV route distance in miles for ordered waypoints (lat, lng).
    /// Returns None if fewer than 2 waypoints or on API error.
    pub async fn calculate_route_miles(&self, waypoints: &[(f64, f64)]) -> Option<f64> {
        if waypoints.len() < 2 { return None; }
        let coordinates: Vec<[f64; 2]> = waypoints.iter()
            .map(|&(lat, lng)| [lng, lat])  // ORS expects [lng, lat]
            .collect();
        let body = OrsRequest { coordinates, units: "mi" };
        let resp = self.client
            .post("https://api.openrouteservice.org/v2/directions/driving-hgv")
            .bearer_auth(&self.api_key)
            .json(&body)
            .timeout(std::time::Duration::from_secs(15))
            .send().await.ok()?;
        if !resp.status().is_success() { return None; }
        let data: OrsResponse = resp.json().await.ok()?;
        data.routes.into_iter().next().map(|r| r.summary.distance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_requires_at_least_two_waypoints() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = RoutingClient::new("fake-key");
        let result = rt.block_on(client.calculate_route_miles(&[(35.1495, -90.0490)]));
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore] // requires valid ORS_API_KEY: cargo test routing -- --ignored
    async fn test_calculate_route_live() {
        let key = std::env::var("ORS_API_KEY").expect("ORS_API_KEY required");
        let client = RoutingClient::new(&key);
        // Memphis, TN → Atlanta, GA
        let miles = client.calculate_route_miles(&[
            (35.1495, -90.0490),
            (33.7490, -84.3880),
        ]).await;
        assert!(miles.is_some());
        let m = miles.unwrap();
        assert!(m > 300.0 && m < 600.0, "expected ~385 miles, got {m}");
    }
}
