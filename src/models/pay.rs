use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The optional per-field overrides carried by a trip or a driver.
#[derive(Debug, Clone, Default)]
pub struct RateOverrides {
    pub loaded_rate_per_mile: Option<f64>,
    pub deadhead_rate_per_mile: Option<f64>,
    pub extra_stop_fee: Option<f64>,
    pub detention_rate_per_hour: Option<f64>,
    pub free_dwell_minutes: Option<u32>,
}

/// The mandatory terminal floor (all concrete).
#[derive(Debug, Clone)]
pub struct TerminalRates {
    pub loaded_rate_per_mile: f64,
    pub deadhead_rate_per_mile: f64,
    pub extra_stop_fee: f64,
    pub detention_rate_per_hour: f64,
    pub free_dwell_minutes: u32,
}

/// Fully resolved, concrete rates used for computation.
#[derive(Debug, Clone, PartialEq)]
pub struct RateSchedule {
    pub loaded_rate_per_mile: f64,
    pub deadhead_rate_per_mile: f64,
    pub extra_stop_fee: f64,
    pub detention_rate_per_hour: f64,
    pub free_dwell_minutes: u32,
}

/// Per-field resolution: trip override ?? driver override ?? terminal floor.
pub fn resolve_rates(trip: &RateOverrides, driver: &RateOverrides, terminal: &TerminalRates)
    -> RateSchedule
{
    RateSchedule {
        loaded_rate_per_mile: trip.loaded_rate_per_mile
            .or(driver.loaded_rate_per_mile).unwrap_or(terminal.loaded_rate_per_mile),
        deadhead_rate_per_mile: trip.deadhead_rate_per_mile
            .or(driver.deadhead_rate_per_mile).unwrap_or(terminal.deadhead_rate_per_mile),
        extra_stop_fee: trip.extra_stop_fee
            .or(driver.extra_stop_fee).unwrap_or(terminal.extra_stop_fee),
        detention_rate_per_hour: trip.detention_rate_per_hour
            .or(driver.detention_rate_per_hour).unwrap_or(terminal.detention_rate_per_hour),
        free_dwell_minutes: trip.free_dwell_minutes
            .or(driver.free_dwell_minutes).unwrap_or(terminal.free_dwell_minutes),
    }
}

/// Minimal per-stop input for pay computation (decouples pay from TripStop).
#[derive(Debug, Clone)]
pub struct PayStopInput {
    pub detention_free_minutes: Option<u32>,
    /// RFC3339 UTC timestamps (use TripStop::actual_arrive_utc/actual_depart_utc).
    pub actual_arrive_utc: Option<String>,
    pub actual_depart_utc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct DriverPay {
    pub loaded_rate_per_mile: f64,
    pub deadhead_rate_per_mile: f64,
    pub loaded_pay: f64,
    pub deadhead_pay: f64,
    pub extra_stop_pay: f64,
    pub detention_pay: f64,
    pub total_pay: f64,
}

fn round2(x: f64) -> f64 { (x * 100.0).round() / 100.0 }

/// Pure pay computation. Detention per stop = max(0, dwell - free)/60 * detention_rate,
/// where free = stop.detention_free_minutes ?? rates.free_dwell_minutes. Stops missing
/// arrive/depart UTC contribute 0. extra_stop_pay = extra_stop_fee * max(0, stop_count - 2).
pub fn compute_driver_pay(
    loaded_miles: Option<f64>,
    deadhead_miles: Option<f64>,
    stops: &[PayStopInput],
    rates: &RateSchedule,
) -> DriverPay {
    let loaded_pay = loaded_miles.unwrap_or(0.0) * rates.loaded_rate_per_mile;
    let deadhead_pay = deadhead_miles.unwrap_or(0.0) * rates.deadhead_rate_per_mile;
    let extra_stops = (stops.len() as i64 - 2).max(0) as f64;
    let extra_stop_pay = extra_stops * rates.extra_stop_fee;

    let mut detention_pay = 0.0;
    for s in stops {
        let (Some(a), Some(d)) = (s.actual_arrive_utc.as_deref(), s.actual_depart_utc.as_deref())
            else { continue };
        let (Ok(at), Ok(dt)) = (
            chrono::DateTime::parse_from_rfc3339(a),
            chrono::DateTime::parse_from_rfc3339(d),
        ) else { continue };
        let dwell_min = (dt - at).num_minutes();
        if dwell_min <= 0 { continue; }
        let free = s.detention_free_minutes.unwrap_or(rates.free_dwell_minutes) as i64;
        let over_min = (dwell_min - free).max(0) as f64;
        detention_pay += (over_min / 60.0) * rates.detention_rate_per_hour;
    }

    let loaded_pay = round2(loaded_pay);
    let deadhead_pay = round2(deadhead_pay);
    let extra_stop_pay = round2(extra_stop_pay);
    let detention_pay = round2(detention_pay);
    DriverPay {
        loaded_rate_per_mile: rates.loaded_rate_per_mile,
        deadhead_rate_per_mile: rates.deadhead_rate_per_mile,
        loaded_pay, deadhead_pay, extra_stop_pay, detention_pay,
        total_pay: round2(loaded_pay + deadhead_pay + extra_stop_pay + detention_pay),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal_floor() -> TerminalRates {
        TerminalRates { loaded_rate_per_mile: 0.50, deadhead_rate_per_mile: 0.40,
            extra_stop_fee: 30.0, detention_rate_per_hour: 20.0, free_dwell_minutes: 120 }
    }

    #[test]
    fn resolves_terminal_floor_when_no_overrides() {
        let r = resolve_rates(&RateOverrides::default(), &RateOverrides::default(), &terminal_floor());
        assert_eq!(r.loaded_rate_per_mile, 0.50);
        assert_eq!(r.free_dwell_minutes, 120);
    }

    #[test]
    fn driver_overrides_terminal_per_field() {
        let driver = RateOverrides { loaded_rate_per_mile: Some(0.60), ..Default::default() };
        let r = resolve_rates(&RateOverrides::default(), &driver, &terminal_floor());
        assert_eq!(r.loaded_rate_per_mile, 0.60);   // from driver
        assert_eq!(r.deadhead_rate_per_mile, 0.40); // still terminal
    }

    #[test]
    fn trip_overrides_driver_and_terminal() {
        let driver = RateOverrides { loaded_rate_per_mile: Some(0.60), detention_rate_per_hour: Some(25.0), ..Default::default() };
        let trip = RateOverrides { loaded_rate_per_mile: Some(0.75), ..Default::default() };
        let r = resolve_rates(&trip, &driver, &terminal_floor());
        assert_eq!(r.loaded_rate_per_mile, 0.75);    // trip wins
        assert_eq!(r.detention_rate_per_hour, 25.0); // driver (no trip override)
        assert_eq!(r.extra_stop_fee, 30.0);          // terminal
    }
}

#[cfg(test)]
mod pay_tests {
    use super::*;

    fn sched() -> RateSchedule {
        RateSchedule { loaded_rate_per_mile: 0.50, deadhead_rate_per_mile: 0.40,
            extra_stop_fee: 30.0, detention_rate_per_hour: 20.0, free_dwell_minutes: 120 }
    }

    fn stop(free: Option<u32>, arrive: Option<&str>, depart: Option<&str>) -> PayStopInput {
        PayStopInput {
            detention_free_minutes: free,
            actual_arrive_utc: arrive.map(|s| s.to_string()),
            actual_depart_utc: depart.map(|s| s.to_string()),
        }
    }

    #[test]
    fn loaded_deadhead_and_extra_stops() {
        let pay = compute_driver_pay(Some(100.0), Some(20.0),
            &[stop(None,None,None), stop(None,None,None), stop(None,None,None), stop(None,None,None)],
            &sched());
        assert_eq!(pay.loaded_pay, 50.0);
        assert_eq!(pay.deadhead_pay, 8.0);
        assert_eq!(pay.extra_stop_pay, 60.0); // 30 * (4-2)
        assert_eq!(pay.detention_pay, 0.0);
        assert_eq!(pay.total_pay, 118.0);
    }

    #[test]
    fn two_or_fewer_stops_no_extra_stop_pay() {
        let pay = compute_driver_pay(Some(10.0), None, &[stop(None,None,None), stop(None,None,None)], &sched());
        assert_eq!(pay.deadhead_pay, 0.0); // deadhead miles None
        assert_eq!(pay.extra_stop_pay, 0.0);
    }

    #[test]
    fn detention_beyond_free_dwell() {
        let pay = compute_driver_pay(Some(0.0), None,
            &[stop(None, Some("2026-05-30T12:00:00+00:00"), Some("2026-05-30T15:00:00+00:00"))],
            &sched());
        assert_eq!(pay.detention_pay, 20.0);
    }

    #[test]
    fn per_stop_free_dwell_override_wins() {
        let pay = compute_driver_pay(Some(0.0), None,
            &[stop(Some(180), Some("2026-05-30T12:00:00+00:00"), Some("2026-05-30T15:00:00+00:00"))],
            &sched());
        assert_eq!(pay.detention_pay, 0.0);
    }

    #[test]
    fn missing_times_contribute_zero_detention() {
        let pay = compute_driver_pay(Some(0.0), None,
            &[stop(None, Some("2026-05-30T12:00:00+00:00"), None)], &sched());
        assert_eq!(pay.detention_pay, 0.0);
    }
}
