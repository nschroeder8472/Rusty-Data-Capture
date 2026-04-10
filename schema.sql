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
    house_total_w    DOUBLE PRECISION,
    house_q          DOUBLE PRECISION,   -- reactive power (VAR)
    house_s          DOUBLE PRECISION,   -- apparent power (VA)
    house_i          DOUBLE PRECISION,   -- current (A)
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

-- Retention policy: keep raw data for 90 days
SELECT add_retention_policy('enphase_readings', INTERVAL '90 days', if_not_exists => TRUE);
SELECT add_retention_policy('tesla_readings', INTERVAL '90 days', if_not_exists => TRUE);
