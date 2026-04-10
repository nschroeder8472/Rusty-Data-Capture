use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use deadpool_postgres::{Config as PgConfig, Pool, Runtime};
use tokio_postgres::NoTls;
use tracing::{info, warn};

use crate::config::Config;
use crate::metrics::{EnphaseReading, SharedState, TeslaReading};

pub fn create_pool(config: &Config) -> Result<Pool> {
    let mut pg_config = PgConfig::new();
    pg_config.url = Some(config.database_url.clone());
    pg_config.pool = Some(deadpool_postgres::PoolConfig::new(config.db_pool_size));

    pg_config
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .context("Failed to create database pool")
}

pub async fn ensure_schema(pool: &Pool) -> Result<()> {
    let client = pool.get().await.context("Failed to get DB connection")?;

    // Create tables and hypertables
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS enphase_readings (
                time             TIMESTAMPTZ      NOT NULL,
                solar_w          DOUBLE PRECISION,
                solar_voltage    DOUBLE PRECISION,
                solar_frequency  DOUBLE PRECISION,
                solar_q          DOUBLE PRECISION,
                solar_s          DOUBLE PRECISION,
                solar_i          DOUBLE PRECISION,
                solar_pf         DOUBLE PRECISION,
                house_total_w    DOUBLE PRECISION,
                house_q          DOUBLE PRECISION,
                house_s          DOUBLE PRECISION,
                house_i          DOUBLE PRECISION,
                grid_net_w       DOUBLE PRECISION,
                grid_q           DOUBLE PRECISION,
                grid_s           DOUBLE PRECISION
            );

            SELECT create_hypertable('enphase_readings', 'time', if_not_exists => TRUE);

            CREATE TABLE IF NOT EXISTS tesla_readings (
                time                TIMESTAMPTZ      NOT NULL,
                charging_w          DOUBLE PRECISION,
                session_wh          DOUBLE PRECISION,
                lifetime_kwh        DOUBLE PRECISION,
                vehicle_connected   BOOLEAN,
                is_charging         BOOLEAN,
                session_s           DOUBLE PRECISION,
                grid_v              DOUBLE PRECISION,
                grid_hz             DOUBLE PRECISION,
                vehicle_current_a   DOUBLE PRECISION,
                evse_state          INTEGER
            );

            SELECT create_hypertable('tesla_readings', 'time', if_not_exists => TRUE);

            CREATE INDEX IF NOT EXISTS idx_enphase_time_desc ON enphase_readings (time DESC);
            CREATE INDEX IF NOT EXISTS idx_tesla_time_desc ON tesla_readings (time DESC);
            CREATE INDEX IF NOT EXISTS idx_tesla_charging ON tesla_readings (time DESC) WHERE is_charging = TRUE;

            CREATE TABLE IF NOT EXISTS gas_prices (
                period           DATE             NOT NULL,
                area_name        TEXT             NOT NULL,
                product_name     TEXT             NOT NULL,
                price_per_gallon DOUBLE PRECISION NOT NULL,
                PRIMARY KEY (period, area_name, product_name)
            );

            CREATE INDEX IF NOT EXISTS idx_gas_prices_period_desc ON gas_prices (period DESC);",
        )
        .await
        .context("Failed to ensure database schema")?;

    // Continuous aggregates must be created individually — they can't be mixed
    // with other statements, and IF NOT EXISTS support varies by TimescaleDB version.
    let aggregates = [
        (
            "enphase_5min",
            "CREATE MATERIALIZED VIEW IF NOT EXISTS enphase_5min
            WITH (timescaledb.continuous) AS
            SELECT
                time_bucket('5 minutes', time) AS bucket,
                avg(solar_w)         AS avg_solar_w,
                avg(house_total_w)   AS avg_house_w,
                avg(grid_net_w)      AS avg_grid_w,
                max(solar_w)         AS peak_solar_w,
                sum(solar_w) / 30    AS solar_wh_5min
            FROM enphase_readings
            GROUP BY bucket;",
        ),
        (
            "enphase_hourly",
            "CREATE MATERIALIZED VIEW IF NOT EXISTS enphase_hourly
            WITH (timescaledb.continuous) AS
            SELECT
                time_bucket('1 hour', time) AS bucket,
                avg(solar_w)         AS avg_solar_w,
                avg(house_total_w)   AS avg_house_w,
                max(solar_w)         AS peak_solar_w,
                sum(solar_w) / 360   AS solar_wh_hourly
            FROM enphase_readings
            GROUP BY bucket;",
        ),
        (
            "tesla_5min",
            "CREATE MATERIALIZED VIEW IF NOT EXISTS tesla_5min
            WITH (timescaledb.continuous) AS
            SELECT
                time_bucket('5 minutes', time) AS bucket,
                avg(charging_w)      AS avg_charging_w,
                max(charging_w)      AS peak_charging_w,
                sum(charging_w) / 30 AS charging_wh_5min,
                bool_or(is_charging) AS any_charging
            FROM tesla_readings
            GROUP BY bucket;",
        ),
        (
            "tesla_hourly",
            "CREATE MATERIALIZED VIEW IF NOT EXISTS tesla_hourly
            WITH (timescaledb.continuous) AS
            SELECT
                time_bucket('1 hour', time) AS bucket,
                avg(charging_w)       AS avg_charging_w,
                max(charging_w)       AS peak_charging_w,
                sum(charging_w) / 360 AS charging_wh_hourly,
                bool_or(is_charging)  AS any_charging
            FROM tesla_readings
            GROUP BY bucket;",
        ),
    ];

    for (name, sql) in &aggregates {
        if let Err(e) = client.batch_execute(sql).await {
            warn!("Could not create continuous aggregate {name}: {e}");
        }
    }

    // Retention policies
    client
        .batch_execute(
            "SELECT add_retention_policy('enphase_readings', INTERVAL '90 days', if_not_exists => TRUE);
            SELECT add_retention_policy('tesla_readings', INTERVAL '90 days', if_not_exists => TRUE);",
        )
        .await
        .context("Failed to set retention policies")?;

    info!("Database schema verified");
    Ok(())
}

pub async fn insert_enphase_reading(
    pool: &Pool,
    reading: &EnphaseReading,
    time: DateTime<Utc>,
) -> Result<()> {
    let client = pool.get().await.context("Failed to get DB connection")?;

    client
        .execute(
            "INSERT INTO enphase_readings (
                time, solar_w, solar_voltage, solar_frequency,
                solar_q, solar_s, solar_i, solar_pf,
                house_total_w, house_q, house_s, house_i,
                grid_net_w, grid_q, grid_s
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
            &[
                &time,
                &reading.solar_w,
                &reading.solar_voltage,
                &reading.solar_frequency,
                &reading.solar_q,
                &reading.solar_s,
                &reading.solar_i,
                &reading.solar_pf,
                &reading.house_total_w,
                &reading.house_q,
                &reading.house_s,
                &reading.house_i,
                &reading.grid_net_w,
                &reading.grid_q,
                &reading.grid_s,
            ],
        )
        .await
        .context("Failed to insert enphase reading")?;

    Ok(())
}

pub async fn insert_tesla_reading(
    pool: &Pool,
    reading: &TeslaReading,
    time: DateTime<Utc>,
) -> Result<()> {
    let client = pool.get().await.context("Failed to get DB connection")?;

    client
        .execute(
            "INSERT INTO tesla_readings (
                time, charging_w, session_wh, lifetime_kwh, vehicle_connected, is_charging,
                session_s, grid_v, grid_hz, vehicle_current_a, evse_state
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            &[
                &time,
                &reading.tesla_w,
                &reading.session_energy_wh,
                &reading.lifetime_kwh,
                &reading.vehicle_connected,
                &reading.is_charging,
                &reading.session_s,
                &reading.grid_v,
                &reading.grid_hz,
                &reading.vehicle_current_a,
                &reading.evse_state,
            ],
        )
        .await
        .context("Failed to insert tesla reading")?;

    Ok(())
}

pub async fn run_writer(
    pool: Pool,
    state: Arc<Mutex<SharedState>>,
    write_interval_secs: u64,
    write_enphase: bool,
    write_tesla: bool,
) {
    let interval = tokio::time::Duration::from_secs(write_interval_secs);

    info!("Database writer started (every {write_interval_secs}s)");

    loop {
        tokio::time::sleep(interval).await;

        let now = Utc::now();

        let (enphase, tesla) = {
            let shared = state.lock().unwrap();
            (shared.enphase.clone(), shared.tesla.clone())
        };

        if write_enphase && enphase.timestamp.is_some() {
            if let Err(e) = insert_enphase_reading(&pool, &enphase, now).await {
                warn!("Enphase write error: {e:#}");
            }
        }

        if write_tesla && tesla.timestamp.is_some() {
            if let Err(e) = insert_tesla_reading(&pool, &tesla, now).await {
                warn!("Tesla write error: {e:#}");
            }
        }
    }
}
