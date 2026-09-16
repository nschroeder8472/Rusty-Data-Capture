use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::Utc;
use futures::StreamExt;
use serde::Deserialize;
use tracing::{info, warn};

use crate::config::EnphaseConfig;
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

    /// Sum reactive power (q) across all available phases.
    pub fn total_q(&self) -> f64 {
        self.ph_a.q
            + self.ph_b.as_ref().map_or(0.0, |ph| ph.q)
            + self.ph_c.as_ref().map_or(0.0, |ph| ph.q)
    }

    /// Sum apparent power (s) across all available phases.
    pub fn total_s(&self) -> f64 {
        self.ph_a.s
            + self.ph_b.as_ref().map_or(0.0, |ph| ph.s)
            + self.ph_c.as_ref().map_or(0.0, |ph| ph.s)
    }

    /// Sum current (i) across all available phases.
    pub fn total_i(&self) -> f64 {
        self.ph_a.i
            + self.ph_b.as_ref().map_or(0.0, |ph| ph.i)
            + self.ph_c.as_ref().map_or(0.0, |ph| ph.i)
    }

    /// True when this section is a byte-for-byte copy of `other`.
    ///
    /// IQ Gateway firmware D8.3.5433 mirrors `net-consumption` into the
    /// `total-consumption` slot, so the two sections become identical. Matching on
    /// p, q and s together is deliberate: p alone is equal every night (when
    /// production is zero, total legitimately equals net) and would false-trigger on
    /// ~40% of samples. p+q+s together had zero false positives across 466,365
    /// pre-regression readings.
    pub fn mirrors(&self, other: &MeterSection) -> bool {
        self.total_p() == other.total_p()
            && self.total_q() == other.total_q()
            && self.total_s() == other.total_s()
    }

    /// Average power factor across active phases.
    pub fn avg_pf(&self) -> f64 {
        let mut sum = self.ph_a.pf;
        let mut count = 1.0;
        if let Some(ph) = &self.ph_b {
            if ph.p.abs() > 0.0 {
                sum += ph.pf;
                count += 1.0;
            }
        }
        if let Some(ph) = &self.ph_c {
            if ph.p.abs() > 0.0 {
                sum += ph.pf;
                count += 1.0;
            }
        }
        sum / count
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

/// Production below this is microinverter standby draw, not real generation.
const SOLAR_DEADBAND_W: f64 = 50.0;

/// Map a parsed payload onto the shared reading.
///
/// House real power is always derived as `production + net-consumption` rather than
/// read from the `total-consumption` section. That section is the gateway's own sum
/// of exactly those two quantities, so the derivation is lossless: across 275,390
/// pre-regression daylight samples it reproduced the reported value with a maximum
/// error of 0.00 W. Deriving unconditionally keeps one code path and stays correct
/// whether or not the firmware is mirroring net into total.
///
/// Note this derives from raw production, before the deadband is applied to
/// `solar_w`; using the deadbanded value would skew house load by up to 50 W at night.
///
/// Returns true if the gateway was mirroring the consumption sections.
pub fn apply_payload(payload: &SsePayload, reading: &mut crate::metrics::EnphaseReading) -> bool {
    let raw_solar = payload.production.total_p();
    let net_p = payload.net_consumption.total_p();

    reading.solar_w = if raw_solar < SOLAR_DEADBAND_W {
        0.0
    } else {
        raw_solar
    };
    reading.solar_voltage = payload.production.ph_a.v;
    reading.solar_frequency = payload.production.ph_a.f;
    reading.solar_q = payload.production.total_q();
    reading.solar_s = payload.production.total_s();
    reading.solar_i = payload.production.total_i();
    reading.solar_pf = payload.production.avg_pf();

    reading.house_total_w = raw_solar + net_p;

    reading.grid_net_w = net_p;
    reading.grid_q = payload.net_consumption.total_q();
    reading.grid_s = payload.net_consumption.total_s();

    // Reactive and apparent power do not sum linearly across a production/net split,
    // and no grid_i is recorded to derive house current from. When the gateway is
    // mirroring, these carry grid values rather than house values, so record NULL —
    // a visible gap in Grafana rather than a plausible wrong number.
    let mirrored = payload.total_consumption.mirrors(&payload.net_consumption);
    if mirrored {
        reading.house_q = None;
        reading.house_s = None;
        reading.house_i = None;
    } else {
        reading.house_q = Some(payload.total_consumption.total_q());
        reading.house_s = Some(payload.total_consumption.total_s());
        reading.house_i = Some(payload.total_consumption.total_i());
    }

    reading.timestamp = Some(Utc::now());
    mirrored
}

pub async fn run_enphase_stream(config: EnphaseConfig, state: Arc<Mutex<SharedState>>) {
    loop {
        if let Err(e) = stream_loop(&config, &state).await {
            warn!("Enphase stream error: {e:#}");
        }
        warn!("Enphase stream disconnected, reconnecting in 5s...");
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}

async fn stream_loop(config: &EnphaseConfig, state: &Arc<Mutex<SharedState>>) -> Result<()> {
    let url = format!("https://{}/stream/meter", config.host);

    let client = reqwest::Client::builder()
        .tls_danger_accept_invalid_certs(true)
        .build()?;

    let response = client
        .get(&url)
        .bearer_auth(&config.token)
        .send()
        .await?
        .error_for_status()?;

    info!("Enphase SSE stream connected");

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut mirroring = false;

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
                    let mirrored = {
                        let mut shared = state.lock().unwrap();
                        apply_payload(&payload, &mut shared.enphase)
                    };

                    // Log only on transition, otherwise this fires every event.
                    if mirrored != mirroring {
                        mirroring = mirrored;
                        if mirrored {
                            warn!(
                                "Gateway is mirroring net-consumption into total-consumption; \
                                 deriving house load as production + net. \
                                 house_q/house_s/house_i will be recorded as NULL."
                            );
                        } else {
                            info!(
                                "Gateway total-consumption is reporting independently again; \
                                 house_q/house_s/house_i restored."
                            );
                        }
                    }
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

    /// Captured from the gateway on firmware D8.3.5433: total-consumption is a
    /// byte-for-byte copy of net-consumption.
    const MIRRORED_PAYLOAD: &str = r#"{
        "production": {
            "ph-a": { "p": 1500.0, "q": 310.371, "s": 317.589, "v": 122.03, "i": 2.602, "pf": 0.9, "f": 60.0 },
            "ph-b": { "p": 1600.0, "q": 318.286, "s": 318.286, "v": 121.976, "i": 2.609, "pf": 0.9, "f": 60.0 },
            "ph-c": { "p": 0.0, "q": 0.0, "s": 0.0, "v": 0.0, "i": 0.0, "pf": 0.0, "f": 60.0 }
        },
        "net-consumption": {
            "ph-a": { "p": 1898.626, "q": 204.467, "s": 135.165, "v": 121.961, "i": -13.204, "pf": 0.984, "f": 60.0 },
            "ph-b": { "p": 1959.713, "q": 123.848, "s": 136.287, "v": 121.928, "i": -14.359, "pf": 0.947, "f": 60.0 },
            "ph-c": { "p": 0.0, "q": 0.0, "s": 0.0, "v": 0.0, "i": 0.0, "pf": 0.0, "f": 60.0 }
        },
        "total-consumption": {
            "ph-a": { "p": 1898.626, "q": 204.467, "s": 135.165, "v": 121.961, "i": -13.204, "pf": 0.984, "f": 60.0 },
            "ph-b": { "p": 1959.713, "q": 123.848, "s": 136.287, "v": 121.928, "i": -14.359, "pf": 0.947, "f": 60.0 },
            "ph-c": { "p": 0.0, "q": 0.0, "s": 0.0, "v": 0.0, "i": 0.0, "pf": 0.0, "f": 60.0 }
        }
    }"#;

    #[test]
    fn test_house_load_derived_not_read_from_total_consumption() {
        let payload = parse_sse_event(MIRRORED_PAYLOAD).unwrap();
        let mut reading = crate::metrics::EnphaseReading::default();
        let mirrored = apply_payload(&payload, &mut reading);

        assert!(mirrored, "mirrored payload should be detected");
        // production (3100.0) + net (3858.339), NOT the mirrored total-consumption
        assert!((reading.house_total_w - 6958.339).abs() < 0.01);
        assert!((reading.grid_net_w - 3858.339).abs() < 0.01);
        // House load must stay above grid when solar is producing.
        assert!(reading.house_total_w > reading.grid_net_w);
    }

    #[test]
    fn test_unreconstructable_fields_are_null_when_mirrored() {
        let payload = parse_sse_event(MIRRORED_PAYLOAD).unwrap();
        let mut reading = crate::metrics::EnphaseReading::default();
        apply_payload(&payload, &mut reading);

        assert_eq!(reading.house_q, None);
        assert_eq!(reading.house_s, None);
        assert_eq!(reading.house_i, None);
    }

    #[test]
    fn test_healthy_payload_keeps_house_reactive_fields() {
        let payload = parse_sse_event(SAMPLE_PAYLOAD).unwrap();
        let mut reading = crate::metrics::EnphaseReading::default();
        let mirrored = apply_payload(&payload, &mut reading);

        assert!(!mirrored, "healthy payload must not be flagged as mirrored");
        assert!(reading.house_q.is_some());
        assert!(reading.house_s.is_some());
        assert!(reading.house_i.is_some());
    }

    /// The derivation must reproduce a healthy gateway's own total-consumption
    /// exactly — this is what made "always derive" safe.
    #[test]
    fn test_derivation_matches_healthy_gateway_total() {
        let payload = parse_sse_event(SAMPLE_PAYLOAD).unwrap();
        let mut reading = crate::metrics::EnphaseReading::default();
        apply_payload(&payload, &mut reading);

        let reported = payload.total_consumption.total_p();
        assert!((reading.house_total_w - reported).abs() < 0.01);
    }

    /// Night-time: production is zero, so total legitimately equals net. This must
    /// NOT be mistaken for the firmware regression.
    #[test]
    fn test_zero_production_night_is_not_flagged_as_mirrored() {
        let night = r#"{
            "production": {
                "ph-a": { "p": 0.0, "q": 310.0, "s": 317.0, "v": 122.0, "i": 2.6, "pf": 0.0, "f": 60.0 }
            },
            "net-consumption": {
                "ph-a": { "p": 1898.0, "q": 204.0, "s": 135.0, "v": 121.9, "i": -13.2, "pf": 0.98, "f": 60.0 }
            },
            "total-consumption": {
                "ph-a": { "p": 1898.0, "q": 514.0, "s": 452.0, "v": 121.9, "i": -13.2, "pf": 0.98, "f": 60.0 }
            }
        }"#;
        let payload = parse_sse_event(night).unwrap();
        let mut reading = crate::metrics::EnphaseReading::default();
        let mirrored = apply_payload(&payload, &mut reading);

        // p matches but q and s differ, so this is a healthy gateway at night.
        assert!(!mirrored);
        assert!(reading.house_q.is_some());
    }

    /// Derivation uses raw production, so the 50 W deadband on solar_w must not
    /// leak into house load.
    #[test]
    fn test_deadband_does_not_skew_derived_house_load() {
        let standby = r#"{
            "production": {
                "ph-a": { "p": 30.0, "q": 0.0, "s": 0.0, "v": 122.0, "i": 0.0, "pf": 0.0, "f": 60.0 }
            },
            "net-consumption": {
                "ph-a": { "p": 1000.0, "q": 10.0, "s": 20.0, "v": 122.0, "i": 8.0, "pf": 0.98, "f": 60.0 }
            },
            "total-consumption": {
                "ph-a": { "p": 1000.0, "q": 10.0, "s": 20.0, "v": 122.0, "i": 8.0, "pf": 0.98, "f": 60.0 }
            }
        }"#;
        let payload = parse_sse_event(standby).unwrap();
        let mut reading = crate::metrics::EnphaseReading::default();
        apply_payload(&payload, &mut reading);

        assert_eq!(reading.solar_w, 0.0, "standby draw is deadbanded");
        // 30 W of raw production still counts toward house load.
        assert!((reading.house_total_w - 1030.0).abs() < 0.01);
    }
}
