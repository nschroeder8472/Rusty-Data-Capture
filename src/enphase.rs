use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::Utc;
use futures::StreamExt;
use serde::Deserialize;
use tracing::{info, warn};

use crate::config::Config;
use crate::metrics::SharedState;

#[derive(Debug, Deserialize)]
pub struct SsePayload {
    pub production: MeterSection,
    #[serde(rename = "net-consumption")]
    pub net_consumption: MeterSection,
    #[serde(rename = "total-consumption")]
    pub total_consumption: MeterSection,
}

#[derive(Debug, Deserialize)]
pub struct MeterSection {
    #[serde(rename = "ph-a")]
    pub ph_a: PhaseData,
    #[serde(rename = "ph-b", default)]
    pub ph_b: Option<PhaseData>,
    #[serde(rename = "ph-c", default)]
    pub ph_c: Option<PhaseData>,
}

impl MeterSection {
    /// Sum real power (p) across all available phases.
    pub fn total_p(&self) -> f64 {
        self.ph_a.p
            + self.ph_b.as_ref().map_or(0.0, |ph| ph.p)
            + self.ph_c.as_ref().map_or(0.0, |ph| ph.p)
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PhaseData {
    pub p: f64,
    pub q: f64,
    pub s: f64,
    pub v: f64,
    pub i: f64,
    pub pf: f64,
    pub f: f64,
}

pub fn parse_sse_event(data: &str) -> Result<SsePayload, serde_json::Error> {
    serde_json::from_str(data)
}

pub async fn run_enphase_stream(config: Config, state: Arc<Mutex<SharedState>>) {
    loop {
        if let Err(e) = stream_loop(&config, &state).await {
            warn!("Enphase stream error: {e:#}");
        }
        warn!("Enphase stream disconnected, reconnecting in 5s...");
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}

async fn stream_loop(config: &Config, state: &Arc<Mutex<SharedState>>) -> Result<()> {
    let url = format!("https://{}/stream/meter", config.envoy_host);

    let client = reqwest::Client::builder()
        .tls_danger_accept_invalid_certs(true)
        .build()?;

    let response = client
        .get(&url)
        .bearer_auth(&config.envoy_token)
        .send()
        .await?
        .error_for_status()?;

    info!("Enphase SSE stream connected");

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        // Normalize \r\n to \n so SSE delimiter detection works
        // regardless of whether the gateway sends \r\n or \n
        let text = String::from_utf8_lossy(&chunk).replace("\r\n", "\n");
        buffer.push_str(&text);

        while let Some(pos) = buffer.find("\n\n") {
            let event = buffer[..pos].to_string();
            buffer = buffer[pos + 2..].to_string();

            let data = event
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .collect::<String>();

            if data.is_empty() {
                continue;
            }

            match parse_sse_event(&data) {
                Ok(payload) => {
                    let mut shared = state.lock().unwrap();
                    shared.enphase.solar_w = payload.production.total_p();
                    shared.enphase.solar_voltage = payload.production.ph_a.v;
                    shared.enphase.solar_frequency = payload.production.ph_a.f;
                    shared.enphase.house_total_w = payload.total_consumption.total_p();
                    shared.enphase.grid_net_w = payload.net_consumption.total_p();
                    shared.enphase.timestamp = Some(Utc::now());
                }
                Err(e) => {
                    warn!("Failed to parse Enphase SSE data: {e}");
                }
            }
        }
    }

    anyhow::bail!("Enphase SSE stream ended unexpectedly");
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_PAYLOAD: &str = r#"{
        "production": {
            "ph-a": { "p": 1170.3, "q": 60.0, "s": 1171.6, "v": 240.1, "i": 4.88, "pf": 0.99, "f": 60.0 },
            "ph-b": { "p": 1170.2, "q": 60.1, "s": 1171.6, "v": 240.0, "i": 4.88, "pf": 0.99, "f": 60.0 },
            "ph-c": { "p": 0.0, "q": 0.0, "s": 0.0, "v": 0.0, "i": 0.0, "pf": 0.0, "f": 0.0 }
        },
        "net-consumption": {
            "ph-a": { "p": 225.1, "q": -40.2, "s": 230.1, "v": 240.1, "i": 0.96, "pf": 0.98, "f": 60.0 },
            "ph-b": { "p": 225.1, "q": -40.1, "s": 230.0, "v": 240.0, "i": 0.96, "pf": 0.98, "f": 60.0 },
            "ph-c": { "p": 0.0, "q": 0.0, "s": 0.0, "v": 0.0, "i": 0.0, "pf": 0.0, "f": 0.0 }
        },
        "total-consumption": {
            "ph-a": { "p": 1395.4, "q": 19.9, "s": 1395.5, "v": 240.1, "i": 5.81, "pf": 0.99, "f": 60.0 },
            "ph-b": { "p": 1395.3, "q": 19.9, "s": 1395.4, "v": 240.0, "i": 5.81, "pf": 0.99, "f": 60.0 },
            "ph-c": { "p": 0.0, "q": 0.0, "s": 0.0, "v": 0.0, "i": 0.0, "pf": 0.0, "f": 0.0 }
        }
    }"#;

    #[test]
    fn test_parse_sse_payload() {
        let payload = parse_sse_event(SAMPLE_PAYLOAD).unwrap();
        assert!((payload.production.ph_a.p - 1170.3).abs() < f64::EPSILON);
        assert!((payload.production.ph_a.v - 240.1).abs() < f64::EPSILON);
        assert!((payload.production.ph_a.f - 60.0).abs() < f64::EPSILON);
        assert!((payload.total_consumption.ph_a.p - 1395.4).abs() < f64::EPSILON);
        assert!((payload.net_consumption.ph_a.p - 225.1).abs() < f64::EPSILON);
    }

    #[test]
    fn test_total_p_sums_all_phases() {
        let payload = parse_sse_event(SAMPLE_PAYLOAD).unwrap();
        let total_production = payload.production.total_p();
        // ph-a (1170.3) + ph-b (1170.2) + ph-c (0.0) = 2340.5
        assert!((total_production - 2340.5).abs() < 0.01);

        let total_consumption = payload.total_consumption.total_p();
        // ph-a (1395.4) + ph-b (1395.3) = 2790.7
        assert!((total_consumption - 2790.7).abs() < 0.01);

        let net_consumption = payload.net_consumption.total_p();
        // ph-a (225.1) + ph-b (225.1) = 450.2
        assert!((net_consumption - 450.2).abs() < 0.01);
    }

    #[test]
    fn test_parse_invalid_json() {
        assert!(parse_sse_event("not json").is_err());
    }
}
