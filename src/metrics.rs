use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Default)]
pub struct EnphaseReading {
    pub solar_w: f64,
    pub solar_voltage: f64,
    pub solar_frequency: f64,
    pub solar_q: f64,
    pub solar_s: f64,
    pub solar_i: f64,
    pub solar_pf: f64,
    pub house_total_w: f64,
    pub house_q: f64,
    pub house_s: f64,
    pub house_i: f64,
    pub grid_net_w: f64,
    pub grid_q: f64,
    pub grid_s: f64,
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct TeslaReading {
    pub tesla_w: f64,
    pub session_energy_wh: f64,
    pub lifetime_kwh: f64,
    pub vehicle_connected: bool,
    pub is_charging: bool,
    pub session_s: f64,
    pub grid_v: f64,
    pub grid_hz: f64,
    pub vehicle_current_a: f64,
    pub evse_state: i32,
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Default)]
pub struct SharedState {
    pub enphase: EnphaseReading,
    pub tesla: TeslaReading,
}
