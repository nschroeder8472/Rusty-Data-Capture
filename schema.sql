-- Solar Energy Monitoring System - Full DDL
-- Run this against a TimescaleDB-enabled PostgreSQL database.

CREATE EXTENSION IF NOT EXISTS timescaledb;

-- Enphase IQ Gateway readings (~8,640 rows/day at 10s interval)
CREATE TABLE IF NOT EXISTS enphase_readings (
    time             TIMESTAMPTZ      NOT NULL,
    solar_w          DOUBLE PRECISION,
    solar_voltage    DOUBLE PRECISION,
    solar_frequency  DOUBLE PRECISION,
    solar_q          DOUBLE PRECISION,   -- reactive power (VAR)
    solar_s          DOUBLE PRECISION,   -- apparent power (VA)
    solar_i          DOUBLE PRECISION,   -- current (A)
    solar_pf         DOUBLE PRECISION,   -- power factor
    -- Derived as raw production + grid_net_w, not read from the gateway's
    -- total-consumption section. Firmware D8.3.5433 mirrors net-consumption into
    -- that section (observed 2026-08-04 onward), which made house load track the
    -- grid and go negative during export.
    house_total_w    DOUBLE PRECISION,
    -- NULL whenever the gateway is mirroring: reactive and apparent power do not
    -- sum linearly across a production/net split, and no grid_i is recorded, so
    -- these cannot be derived. NULL is deliberate — a visible gap beats a
    -- plausible wrong number.
    house_q          DOUBLE PRECISION,   -- reactive power (VAR), nullable
    house_s          DOUBLE PRECISION,   -- apparent power (VA), nullable
    house_i          DOUBLE PRECISION,   -- current (A), nullable
    grid_net_w       DOUBLE PRECISION,   -- negative = exporting
    grid_q           DOUBLE PRECISION,   -- reactive power (VAR)
    grid_s           DOUBLE PRECISION    -- apparent power (VA)
);

SELECT create_hypertable('enphase_readings', 'time', if_not_exists => TRUE);

-- Tesla Wall Connector readings (~8,640 rows/day at 10s interval)
CREATE TABLE IF NOT EXISTS tesla_readings (
    time                TIMESTAMPTZ      NOT NULL,
    charging_w          DOUBLE PRECISION,
    session_wh          DOUBLE PRECISION,
    lifetime_kwh        DOUBLE PRECISION,
    vehicle_connected   BOOLEAN,
    is_charging         BOOLEAN,
    session_s           DOUBLE PRECISION,   -- session duration (seconds)
    grid_v              DOUBLE PRECISION,   -- grid voltage
    grid_hz             DOUBLE PRECISION,   -- grid frequency
    vehicle_current_a   DOUBLE PRECISION,   -- total vehicle current (A)
    evse_state          INTEGER             -- EVSE state code
);

SELECT create_hypertable('tesla_readings', 'time', if_not_exists => TRUE);

-- Gasoline price data (weekly from EIA)
CREATE TABLE IF NOT EXISTS gas_prices (
    period           DATE             NOT NULL,
    area_name        TEXT             NOT NULL,
    product_name     TEXT             NOT NULL,
    price_per_gallon DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (period, area_name, product_name)
);

CREATE INDEX IF NOT EXISTS idx_gas_prices_period_desc ON gas_prices (period DESC);

-- Indexes for common queries
CREATE INDEX IF NOT EXISTS idx_enphase_time_desc ON enphase_readings (time DESC);
CREATE INDEX IF NOT EXISTS idx_tesla_time_desc ON tesla_readings (time DESC);
CREATE INDEX IF NOT EXISTS idx_tesla_charging ON tesla_readings (time DESC) WHERE is_charging = TRUE;

-- Continuous aggregates: Enphase 5-minute averages
CREATE MATERIALIZED VIEW IF NOT EXISTS enphase_5min
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('5 minutes', time) AS bucket,
    avg(solar_w)         AS avg_solar_w,
    avg(house_total_w)   AS avg_house_w,
    avg(grid_net_w)      AS avg_grid_w,
    max(solar_w)         AS peak_solar_w,
    sum(solar_w) / 30    AS solar_wh_5min   -- 30 samples at 10s = 5 min
FROM enphase_readings
GROUP BY bucket;

-- Continuous aggregates: Enphase hourly rollups
CREATE MATERIALIZED VIEW IF NOT EXISTS enphase_hourly
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', time) AS bucket,
    avg(solar_w)         AS avg_solar_w,
    avg(house_total_w)   AS avg_house_w,
    max(solar_w)         AS peak_solar_w,
    sum(solar_w) / 360   AS solar_wh_hourly  -- 360 samples at 10s = 1 hour
FROM enphase_readings
GROUP BY bucket;

-- Continuous aggregates: Tesla 5-minute averages
CREATE MATERIALIZED VIEW IF NOT EXISTS tesla_5min
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('5 minutes', time) AS bucket,
    avg(charging_w)      AS avg_charging_w,
    max(charging_w)      AS peak_charging_w,
    sum(charging_w) / 30 AS charging_wh_5min,
    bool_or(is_charging) AS any_charging
FROM tesla_readings
GROUP BY bucket;

-- Continuous aggregates: Tesla hourly rollups
CREATE MATERIALIZED VIEW IF NOT EXISTS tesla_hourly
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', time) AS bucket,
    avg(charging_w)       AS avg_charging_w,
    max(charging_w)       AS peak_charging_w,
    sum(charging_w) / 360 AS charging_wh_hourly,
    bool_or(is_charging)  AS any_charging
FROM tesla_readings
GROUP BY bucket;

-- Refresh policies: without these a continuous aggregate only updates when
-- refreshed by hand and silently goes stale (tesla_5min and tesla_hourly sat
-- frozen from 2026-04-06 to 2026-09-17 for exactly this reason).
--
-- start_offset stays far short of the retention window below. Refreshing a
-- window whose raw rows have already been dropped would delete the materialized
-- history for that window — the very thing these aggregates exist to preserve.
SELECT add_continuous_aggregate_policy('enphase_5min',
    start_offset => INTERVAL '1 day', end_offset => INTERVAL '10 minutes',
    schedule_interval => INTERVAL '5 minutes', if_not_exists => TRUE);
SELECT add_continuous_aggregate_policy('enphase_hourly',
    start_offset => INTERVAL '7 days', end_offset => INTERVAL '1 hour',
    schedule_interval => INTERVAL '30 minutes', if_not_exists => TRUE);
SELECT add_continuous_aggregate_policy('tesla_5min',
    start_offset => INTERVAL '1 day', end_offset => INTERVAL '10 minutes',
    schedule_interval => INTERVAL '5 minutes', if_not_exists => TRUE);
SELECT add_continuous_aggregate_policy('tesla_hourly',
    start_offset => INTERVAL '7 days', end_offset => INTERVAL '1 hour',
    schedule_interval => INTERVAL '30 minutes', if_not_exists => TRUE);

-- Retention policy: keep raw data for 2 years (~1.2 GB/year for both tables).
-- Older data survives in the continuous aggregates above, which have no retention.
--
-- Dropped first because add_retention_policy(if_not_exists => TRUE) is a no-op
-- when a policy already exists — it does NOT update the interval. Without the
-- drop, changing this value here would silently never reach an existing database.
SELECT remove_retention_policy('enphase_readings', if_exists => TRUE);
SELECT remove_retention_policy('tesla_readings', if_exists => TRUE);
SELECT add_retention_policy('enphase_readings', INTERVAL '2 years', if_not_exists => TRUE);
SELECT add_retention_policy('tesla_readings', INTERVAL '2 years', if_not_exists => TRUE);
