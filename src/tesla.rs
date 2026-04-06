use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use tracing::{info, warn};

use crate::config::Config;
use crate::metrics::SharedState;

#[derive(Debug, Deserialize)]
pub struct TeslaVitals {
    pub contactor_closed: bool,
    pub vehicle_connected: bool,
    pub session_energy_wh: f64,
    #[serde(rename = "voltageA_v")]
    pub voltage_a_v: f64,
    #[serde(rename = "voltageB_v")]
    pub voltage_b_v: f64,
    #[serde(rename = "currentA_a")]
    pub current_a_a: f64,
    #[serde(rename = "currentB_a")]
    pub current_b_a: f64,
    #[serde(default)]
    pub session_s: f64,
    #[serde(default)]
    pub grid_v: f64,
    #[serde(default)]
    pub grid_hz: f64,
    #[serde(default)]
    pub vehicle_current_a: f64,
    #[serde(default)]
    pub evse_state: i32,
}

impl TeslaVitals {
    pub fn charging_power_w(&self) -> f64 {
        (self.voltage_a_v * self.current_a_a) + (self.voltage_b_v * self.current_b_a)
    }
}

pub async fn run_tesla_poller(config: Config, state: Arc<Mutex<SharedState>>) {
    let client = reqwest::Client::new();
    let vitals_url = format!("http://{}/api/1/vitals", config.tesla_host);
    let lifetime_url = format!("http://{}/api/1/lifetime", config.tesla_host);
    let interval = tokio::time::Duration::from_secs(config.tesla_poll_interval_secs);

    info!("Tesla poller started (every {}s)", config.tesla_poll_interval_secs);

    loop {
        match poll_tesla(&client, &vitals_url, &lifetime_url, &state).await {
            Ok(()) => {}
            Err(e) => {
                warn!("Tesla poll error: {e:#}");
            }
        }
        tokio::time::sleep(interval).await;
    }
}

async fn poll_tesla(
    client: &reqwest::Client,
    vitals_url: &str,
    lifetime_url: &str,
    state: &Arc<Mutex<SharedState>>,
) -> Result<()> {
    let vitals: TeslaVitals = client.get(vitals_url).send().await?.json().await?;

    let lifetime_kwh = match client.get(lifetime_url).send().await {
        Ok(resp) => {
            #[derive(Deserialize)]
            struct Lifetime {
                energy_wh: f64,
            }
            resp.json::<Lifetime>()
                .await
                .map(|l| l.energy_wh / 1000.0)
                .unwrap_or(0.0)
        }
        Err(_) => 0.0,
    };

    let mut shared = state.lock().unwrap();
    shared.tesla.tesla_w = vitals.charging_power_w();
    shared.tesla.session_energy_wh = vitals.session_energy_wh;
    shared.tesla.lifetime_kwh = lifetime_kwh;
    shared.tesla.vehicle_connected = vitals.vehicle_connected;
    shared.tesla.is_charging = vitals.contactor_closed;
    shared.tesla.session_s = vitals.session_s;
    shared.tesla.grid_v = vitals.grid_v;
    shared.tesla.grid_hz = vitals.grid_hz;
    shared.tesla.vehicle_current_a = vitals.vehicle_current_a;
    shared.tesla.evse_state = vitals.evse_state;
    shared.tesla.timestamp = Some(Utc::now());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_vitals() {
        let json = r#"{
            "contactor_closed": true,
            "vehicle_connected": true,
            "session_s": 3600,
            "grid_v": 240.5,
            "grid_hz": 60.0,
            "vehicle_current_a": 32.0,
            "voltageA_v": 120.2,
            "voltageB_v": 120.3,
            "voltageN_v": 0.0,
            "currentA_a": 16.0,
            "currentB_a": 16.0,
            "currentN_a": 0.0,
            "session_energy_wh": 5000,
            "config_status": 5,
            "evse_state": 5,
            "current_alerts": []
        }"#;

        let vitals: TeslaVitals = serde_json::from_str(json).unwrap();
        assert!(vitals.contactor_closed);
        assert!(vitals.vehicle_connected);
        assert!((vitals.session_energy_wh - 5000.0).abs() < f64::EPSILON);

        let power = vitals.charging_power_w();
        let expected = (120.2 * 16.0) + (120.3 * 16.0);
        assert!((power - expected).abs() < 0.01);
    }

    #[test]
    fn test_zero_current_zero_power() {
        let vitals = TeslaVitals {
            contactor_closed: false,
            vehicle_connected: true,
            session_energy_wh: 0.0,
            voltage_a_v: 120.2,
            voltage_b_v: 120.3,
            current_a_a: 0.0,
            current_b_a: 0.0,
            session_s: 0.0,
            grid_v: 240.5,
            grid_hz: 60.0,
            vehicle_current_a: 0.0,
            evse_state: 0,
        };
        assert_eq!(vitals.charging_power_w(), 0.0);
    }
}
