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
    #[serde(default)]
    segments: Vec<OrsSegment>,
}

#[derive(Deserialize)]
struct OrsSummary {
    distance: f64,
}

#[derive(Deserialize)]
struct OrsSegment {
    distance: f64,
}

#[derive(Debug, Clone)]
pub struct RouteMiles {
    pub total_miles: f64,
    pub segment_miles: Vec<f64>,
}

impl RoutingClient {
    pub fn new(api_key: &str) -> Self {
        Self { client: Client::new(), api_key: api_key.to_string() }
    }

    /// Calculates HGV route distances for ordered waypoints (lat, lng).
    /// Returns total miles and per-segment miles (one entry per consecutive pair).
    /// Returns None if fewer than 2 waypoints or on API error.
    pub async fn calculate_route_with_segments(
        &self, waypoints: &[(f64, f64)],
    ) -> Option<RouteMiles> {
        if waypoints.len() < 2 { return None; }
        let coordinates: Vec<[f64; 2]> = waypoints.iter()
            .map(|&(lat, lng)| [lng, lat])
            .collect();
        let body = OrsRequest { coordinates, units: "mi" };
        let resp = self.client
            .post("https://api.heigit.org/openrouteservice/v2/directions/driving-hgv")
            .bearer_auth(&self.api_key)
            .json(&body)
            .timeout(std::time::Duration::from_secs(15))
            .send().await.ok()?;
        if !resp.status().is_success() { return None; }
        let data: OrsResponse = resp.json().await.ok()?;
        let route = data.routes.into_iter().next()?;
        let segment_miles: Vec<f64> = route.segments.iter().map(|s| s.distance).collect();
        Some(RouteMiles {
            total_miles: route.summary.distance,
            segment_miles,
        })
    }

    /// Calculates total HGV route distance in miles for ordered waypoints (lat, lng).
    /// Returns None if fewer than 2 waypoints or on API error.
    pub async fn calculate_route_miles(&self, waypoints: &[(f64, f64)]) -> Option<f64> {
        self.calculate_route_with_segments(waypoints).await.map(|r| r.total_miles)
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
    async fn test_calculate_route_with_segments_live() {
        let key = std::env::var("ORS_API_KEY").expect("ORS_API_KEY required");
        let client = RoutingClient::new(&key);
        // 3 waypoints: Memphis → Nashville → Atlanta
        let route = client.calculate_route_with_segments(&[
            (35.1495, -90.0490),
            (36.1627, -86.7816),
            (33.7490, -84.3880),
        ]).await;
        assert!(route.is_some());
        let r = route.unwrap();
        assert_eq!(r.segment_miles.len(), 2);
        assert!(r.segment_miles[0] > 100.0 && r.segment_miles[0] < 300.0, "leg 1 mi: {}", r.segment_miles[0]);
        assert!(r.segment_miles[1] > 150.0 && r.segment_miles[1] < 350.0, "leg 2 mi: {}", r.segment_miles[1]);
        assert!((r.total_miles - r.segment_miles.iter().sum::<f64>()).abs() < 0.5);
    }

    #[test]
    fn test_calculate_route_with_segments_requires_two_waypoints() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = RoutingClient::new("fake-key");
        let result = rt.block_on(client.calculate_route_with_segments(&[(35.1495, -90.0490)]));
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
