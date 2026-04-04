# Solar Energy Monitoring System - Design Document

**Version:** 1.0  
**Date:** February 21, 2026  
**Author:** System Design  

---

## 1. Executive Summary

This document outlines the design for a real-time solar energy monitoring system that tracks Enphase solar panel production, Tesla Wall Connector charging data, and calculates cost savings metrics. The system provides a Grafana dashboard for visualization and historical analysis.

### 1.1 Goals

- **Real-time monitoring** of solar production at 1-second resolution
- **Track energy flows** between solar panels, home consumption, grid, and Tesla charging
- **Calculate cost savings** from solar offset and EV vs. gasoline fuel costs
- **Historical analysis** with unlimited query time range (months/years of data)
- **Minimal resource footprint** using efficient compiled binaries

### 1.2 Non-Goals

- Home automation control (read-only monitoring)
- Mobile app (Grafana web UI only)
- Multi-site monitoring (single home installation)

---

## 2. System Architecture

### 2.1 High-Level Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Data Sources (LAN)                      │
├──────────────────────────┬──────────────────────────────────┤
│  Enphase IQ Gateway      │  Tesla Wall Connector Gen 3     │
│  (envoy.local)           │  (192.168.1.x)                  │
│  • SSE stream @ 1/sec    │  • HTTP polling @ 10s           │
│  • /stream/meter         │  • /api/1/vitals                │
└──────────────────────────┴──────────────────────────────────┘
                             │
                             ▼
              ┌──────────────────────────────┐
              │   Rust Data Collector        │
              │   • Async/Tokio runtime      │
              │   • Independent tasks        │
              │   • Automatic retry logic    │
              │   • Batched writes           │
              └──────────────────────────────┘
                             │
                             ▼
              ┌──────────────────────────────┐
              │   TimescaleDB                │
              │   (PostgreSQL + Extension)   │
              │   • Hypertable: energy       │
              │   • Continuous aggregates    │
              │   • Retention policies       │
              └──────────────────────────────┘
                             │
                             ▼
              ┌──────────────────────────────┐
              │   Grafana Dashboard          │
              │   • Real-time panels         │
              │   • Cost calculations        │
              │   • Historical trends        │
              └──────────────────────────────┘
```

### 2.2 Component Overview

| Component | Purpose | Technology | Location |
|-----------|---------|------------|----------|
| **Enphase IQ Gateway** | Solar production + house consumption metering | Enphase firmware | Physical device on LAN |
| **Tesla Wall Connector** | EV charging metrics | Tesla firmware | Physical device on LAN |
| **Data Collector** | Scrape APIs, compute metrics, write to DB | Rust binary | Raspberry Pi / NAS / Server |
| **TimescaleDB** | Time-series storage with PostgreSQL features | PostgreSQL 14+ with TimescaleDB extension | Same host or dedicated |
| **Grafana** | Visualization and dashboards | Grafana OSS | Same host or dedicated |

---

## 3. Data Sources

### 3.1 Enphase IQ Gateway

**Endpoint:** `https://envoy.local/stream/meter`  
**Protocol:** Server-Sent Events (SSE) — persistent HTTP connection with streaming JSON  
**Frequency:** ~1 update per second  
**Authentication:** Bearer token (JWT from entrez.enphaseenergy.com, valid 1 year)

#### Data Structure

```json
{
  "production": {
    "ph-a": { "p": 2340.5, "q": 120.1, "s": 2343.2, "v": 240.1, "i": 9.76, "pf": 0.99, "f": 60.0 },
    "ph-b": { "p": 0.0, "q": 0.0, "s": 0.0, "v": 0.0, "i": 0.0, "pf": 0.0, "f": 0.0 },
    "ph-c": { "p": 0.0, "q": 0.0, "s": 0.0, "v": 0.0, "i": 0.0, "pf": 0.0, "f": 0.0 }
  },
  "net-consumption": {
    "ph-a": { "p": 450.2, "q": -80.3, "s": 460.1, "v": 240.1, "i": 1.92, "pf": 0.98, "f": 60.0 },
    "ph-b": { "p": 0.0, ... },
    "ph-c": { "p": 0.0, ... }
  },
  "total-consumption": {
    "ph-a": { "p": 2790.7, "q": 39.8, "s": 2790.9, "v": 240.1, "i": 11.62, "pf": 0.99, "f": 60.0 },
    "ph-b": { "p": 0.0, ... },
    "ph-c": { "p": 0.0, ... }
  }
}
```

**Field Definitions:**
- `p`: Real power (watts) — **primary metric**
- `q`: Reactive power (VAR)
- `s`: Apparent power (VA)
- `v`: Voltage (V)
- `i`: Current (A)
- `pf`: Power factor
- `f`: Frequency (Hz)

**Phase Configuration:** Single-phase installation uses `ph-a` only; `ph-b` and `ph-c` are zero.

**Metrics Extracted:**
- `production.ph-a.p` → Solar generation (watts)
- `total-consumption.ph-a.p` → Total house load including Tesla (watts)
- `net-consumption.ph-a.p` → Grid import/export (positive = importing, negative = exporting)

### 3.2 Tesla Wall Connector Gen 3

**Endpoint:** `http://<tesla-ip>/api/1/vitals`  
**Protocol:** HTTP GET, JSON response  
**Frequency:** Poll every 10 seconds  
**Authentication:** None (unauthenticated on LAN)

#### Data Structure

```json
{
  "contactor_closed": false,
  "vehicle_connected": true,
  "session_s": 0,
  "grid_v": 240.5,
  "grid_hz": 60.0,
  "vehicle_current_a": 0.0,
  "voltageA_v": 120.2,
  "voltageB_v": 120.3,
  "voltageN_v": 0.0,
  "currentA_a": 0.0,
  "currentB_a": 0.0,
  "currentN_a": 0.0,
  "session_energy_wh": 0,
  "config_status": 5,
  "evse_state": 5,
  "current_alerts": []
}
```

**Key Fields:**
- `contactor_closed` → Actively charging (boolean)
- `vehicle_connected` → Car plugged in (boolean)
- `session_energy_wh` → Energy delivered in current session (Wh)
- `voltageA_v`, `voltageB_v` → Split-phase 240V legs
- `currentA_a`, `currentB_a` → Current draw per phase

**Charging Power Calculation:**
```
tesla_charging_watts = (voltageA_v × currentA_a) + (voltageB_v × currentB_a)
```

**Additional Endpoint (Lifetime Stats):**
`http://<tesla-ip>/api/1/lifetime` provides:
- `energy_wh`: Cumulative lifetime energy delivered
- `charge_starts`: Number of charging sessions
- `connector_cycles`: Physical plug/unplug cycles

---

## 4. Data Model

### 4.1 TimescaleDB Schema

```sql
-- Create extension
CREATE EXTENSION IF NOT EXISTS timescaledb;

-- Main hypertable for all energy metrics
CREATE TABLE energy (
  time            TIMESTAMPTZ NOT NULL,
  
  -- Enphase raw metrics
  solar_w         DOUBLE PRECISION,
  solar_voltage   DOUBLE PRECISION,
  solar_frequency DOUBLE PRECISION,
  house_total_w   DOUBLE PRECISION,
  grid_net_w      DOUBLE PRECISION,  -- negative = exporting
  
  -- Tesla raw metrics
  tesla_w                DOUBLE PRECISION DEFAULT 0,
  tesla_session_wh       DOUBLE PRECISION DEFAULT 0,
  tesla_lifetime_kwh     DOUBLE PRECISION,
  tesla_vehicle_connected SMALLINT DEFAULT 0,  -- boolean as 0/1
  tesla_is_charging      SMALLINT DEFAULT 0,
  
  -- Derived metrics (computed by Rust before insert)
  house_excl_tesla_w        DOUBLE PRECISION,  -- house_total_w - tesla_w
  net_solar_vs_total_w      DOUBLE PRECISION,  -- solar_w - house_total_w
  net_solar_vs_house_only_w DOUBLE PRECISION   -- solar_w - house_excl_tesla_w
);

-- Convert to hypertable (TimescaleDB magic)
SELECT create_hypertable('energy', 'time');

-- Indexes for common queries
CREATE INDEX idx_energy_time_desc ON energy (time DESC);
CREATE INDEX idx_energy_charging ON energy (time DESC) 
  WHERE tesla_is_charging = 1;

-- Continuous aggregate: 5-minute averages
CREATE MATERIALIZED VIEW energy_5min
WITH (timescaledb.continuous) AS
SELECT 
  time_bucket('5 minutes', time) AS bucket,
  avg(solar_w) AS avg_solar_w,
  avg(house_total_w) AS avg_house_w,
  avg(tesla_w) AS avg_tesla_w,
  avg(grid_net_w) AS avg_grid_w,
  max(solar_w) AS peak_solar_w,
  max(tesla_w) AS peak_tesla_w,
  sum(solar_w) / 12 AS solar_wh_5min  -- watts to watt-hours (5min = 1/12 hour)
FROM energy
GROUP BY bucket;

-- Continuous aggregate: hourly rollups
CREATE MATERIALIZED VIEW energy_hourly
WITH (timescaledb.continuous) AS
SELECT 
  time_bucket('1 hour', time) AS bucket,
  avg(solar_w) AS avg_solar_w,
  avg(house_total_w) AS avg_house_w,
  avg(tesla_w) AS avg_tesla_w,
  max(solar_w) AS peak_solar_w,
  sum(solar_w) / 60 AS solar_wh_hourly,  -- approximate Wh from avg watts
  sum(tesla_w) / 60 AS tesla_wh_hourly
FROM energy
GROUP BY bucket;

-- Retention policy: keep raw data for 90 days, aggregates forever
SELECT add_retention_policy('energy', INTERVAL '90 days');
-- 5-min and hourly aggregates retained indefinitely
```

### 4.2 Data Retention Strategy

| Resolution | Retention | Storage Use |
|------------|-----------|-------------|
| **Raw (1s)** | 90 days | ~7.8 GB/year (estimated) |
| **5-minute** | Forever | ~140 MB/year |
| **Hourly** | Forever | ~12 MB/year |

---

## 5. Application Design

### 5.1 Rust Data Collector

**Architecture:** Multi-task async application using Tokio runtime

```
┌─────────────────────────────────────────────────────┐
│                   main.rs                           │
│   • Load config from .env                           │
│   • Initialize PostgreSQL connection pool           │
│   • Spawn independent tasks                         │
│   • Error handling and restart logic                │
└─────────────────────────────────────────────────────┘
              │
              ├─────────────────────────────────────┐
              │                                     │
        ┌─────▼─────┐                      ┌───────▼──────┐
        │ Enphase   │                      │ Tesla        │
        │ Streamer  │                      │ Poller       │
        │           │                      │              │
        │ • SSE     │                      │ • 10s timer  │
        │ • Parse   │                      │ • Parse JSON │
        │ • Retry   │                      │ • Retry      │
        └─────┬─────┘                      └───────┬──────┘
              │                                     │
              └──────────┬──────────────────────────┘
                         │
                  ┌──────▼─────────┐
                  │ Shared State   │
                  │ (Arc<Mutex>)   │
                  │                │
                  │ Last Enphase + │
                  │ Last Tesla     │
                  └──────┬─────────┘
                         │
                  ┌──────▼─────────┐
                  │ Batch Writer   │
                  │                │
                  │ • Combines     │
                  │ • Computes     │
                  │ • Inserts      │
                  └────────────────┘
```

### 5.2 Key Rust Modules

```rust
// src/main.rs
// Entry point, spawns tasks

// src/config.rs
// Environment variable loading and validation

// src/enphase.rs
// SSE stream handling for /stream/meter
// Struct: EnphaseMetrics

// src/tesla.rs
// HTTP polling for /api/1/vitals
// Struct: TeslaMetrics

// src/metrics.rs
// Shared state struct combining Enphase + Tesla
// Derived metric calculations
// Struct: CombinedMetrics

// src/database.rs
// TimescaleDB connection pool
// Batch insert logic
// SQL query definitions

// src/error.rs
// Custom error types using thiserror
```

### 5.3 Dependencies (Cargo.toml)

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
tokio-postgres = "0.7"
deadpool-postgres = "0.14"  # Connection pooling
reqwest = { version = "0.12", features = ["rustls-tls", "stream", "json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
futures = "0.3"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
dotenvy = "0.15"
anyhow = "1"
thiserror = "1"
chrono = "0.4"
```

### 5.4 Configuration (.env)

```bash
# Enphase IQ Gateway
ENVOY_HOST=envoy.local
ENVOY_TOKEN=eyJ0eXAiOiJKV1QiLCJhbGc...  # From entrez.enphaseenergy.com

# Tesla Wall Connector
TESLA_HOST=192.168.1.75
TESLA_POLL_INTERVAL_SECS=10

# TimescaleDB
DATABASE_URL=postgresql://user:password@localhost:5432/solar
DB_POOL_SIZE=5

# Cost Calculation (for derived metrics)
ELECTRIC_RATE_PER_KWH=0.16      # $/kWh
GAS_PRICE_PER_GALLON=3.80       # $/gallon
TESLA_MILES_PER_KWH=3.5         # Model 3 LR efficiency
ICE_MPG=30.0                    # Comparison vehicle

# Logging
RUST_LOG=info  # debug, info, warn, error
```

---

## 6. Metrics and Calculations

### 6.1 Primary Metrics (Raw)

| Metric | Source | Unit | Description |
|--------|--------|------|-------------|
| `solar_w` | Enphase production | W | Current solar generation |
| `house_total_w` | Enphase total-consumption | W | Total house load (includes Tesla) |
| `grid_net_w` | Enphase net-consumption | W | Grid exchange (+ import, - export) |
| `tesla_w` | Tesla vitals calculated | W | Instantaneous charging power |
| `tesla_session_wh` | Tesla vitals | Wh | Energy in current charge session |

### 6.2 Derived Metrics (Computed)

| Metric | Formula | Description |
|--------|---------|-------------|
| `house_excl_tesla_w` | `house_total_w - tesla_w` | House consumption without EV charging |
| `net_solar_vs_total_w` | `solar_w - house_total_w` | Surplus/deficit including Tesla |
| `net_solar_vs_house_only_w` | `solar_w - house_excl_tesla_w` | Surplus/deficit excluding Tesla |

### 6.3 Cost Calculations (Grafana Layer)

These are computed in Grafana queries, not stored:

**Daily Solar Savings:**
```sql
SELECT 
  time_bucket('1 day', time) AS day,
  (sum(solar_w) / 3600) * 0.16 AS daily_solar_savings_usd
FROM energy
WHERE time > now() - interval '30 days'
GROUP BY day;
```

**Tesla Fuel Savings (vs. Gasoline):**
```sql
SELECT 
  time_bucket('1 day', time) AS day,
  -- Energy used for charging (kWh)
  sum(tesla_w) / 3600 / 1000 AS kwh_charged,
  -- Equivalent miles driven
  (sum(tesla_w) / 3600 / 1000) * 3.5 AS miles_driven,
  -- Gasoline cost equivalent
  ((sum(tesla_w) / 3600 / 1000) * 3.5 / 30.0) * 3.80 AS gas_cost,
  -- Electricity cost
  (sum(tesla_w) / 3600 / 1000) * 0.16 AS elec_cost,
  -- Net savings
  (((sum(tesla_w) / 3600 / 1000) * 3.5 / 30.0) * 3.80) - 
  ((sum(tesla_w) / 3600 / 1000) * 0.16) AS net_savings_usd
FROM energy
WHERE tesla_is_charging = 1
  AND time > now() - interval '30 days'
GROUP BY day;
```

---

## 7. Grafana Dashboard Design

### 7.1 Dashboard Layout

```
┌─────────────────────────────────────────────────────────────┐
│  Solar Energy Monitor              Last update: 2s ago      │
├──────────────────────┬──────────────────┬───────────────────┤
│ Solar Generation     │ House Load       │ Tesla Charging    │
│ 2.4 kW              │ 1.8 kW           │ 0 kW              │
│ ████████░░░░ 82%    │                  │ Not charging      │
├──────────────────────┴──────────────────┴───────────────────┤
│ Real-Time Power Flow (Last 6 Hours)                         │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │        ▲Solar                                           │ │
│ │    ┌───┼───┐                                            │ │
│ │ 3kW│   │   │                                            │ │
│ │    │   │   │                                            │ │
│ │ 2kW├───┤   ├───House (excl Tesla)                      │ │
│ │    │   │   │                                            │ │
│ │ 1kW│   │   │                                            │ │
│ │    │   ▼   │                                            │ │
│ │ 0kW├───────┼───Grid (+ import / - export)              │ │
│ │    │       │                                            │ │
│ │-1kW│       │                                            │ │
│ │    └───────┴────────────────────────────────────────────┤ │
│ │      6h    5h    4h    3h    2h    1h    now           │ │
│ └─────────────────────────────────────────────────────────┘ │
├──────────────────────┬──────────────────────────────────────┤
│ Net Solar vs Total   │ Net Solar vs House Only              │
│ (incl. Tesla)        │ (excl. Tesla)                        │
│ ┌──────────────────┐ │ ┌──────────────────┐                │
│ │ + export zone    │ │ │ Green = surplus  │                │
│ │ ════════════════ │ │ │ ════════════════ │                │
│ │ - import zone    │ │ │ Red = deficit    │                │
│ └──────────────────┘ │ └──────────────────┘                │
├──────────────────────┴──────────────────────────────────────┤
│ Cost Savings Analysis (Last 30 Days)                        │
│ ┌──────────────────┬────────────────────────────────────┐  │
│ │ Solar Offset     │ Tesla Fuel Savings                 │  │
│ │ ┌──────────────┐ │ ┌────────────────────────────────┐ │  │
│ │ │ $45.20       │ │ │ Gas equiv: $67.80              │ │  │
│ │ │ (This month) │ │ │ Elec cost: $15.30              │ │  │
│ │ │              │ │ │ Net saved: $52.50              │ │  │
│ │ └──────────────┘ │ └────────────────────────────────┘ │  │
│ └──────────────────┴────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 7.2 Panel Specifications

**Panel 1: Solar Generation (Stat + Time Series)**
- Query: `SELECT solar_w FROM energy WHERE $__timeFilter(time)`
- Visualization: Stat (big number) + Area graph
- Thresholds: 0% (red), 30% (yellow), 70% (green) of system max
- Time range: Last 6 hours (configurable)

**Panel 2: House Load (Stat)**
- Query: `SELECT house_total_w FROM energy WHERE $__timeFilter(time)`
- Display: Current value in kW
- Color: Based on % of main breaker rating

**Panel 3: Tesla Charging (Stat)**
- Query: `SELECT tesla_w, tesla_is_charging FROM energy WHERE $__timeFilter(time) ORDER BY time DESC LIMIT 1`
- Display: "Charging X kW" or "Not charging"
- Icon changes based on state

**Panel 4: Power Flow Time Series**
- Query combines 4 series: solar, house (excl Tesla), grid, Tesla
- Stacked area chart
- Legend shows current values

**Panel 5: Net Solar vs Total (Positive/Negative Fill)**
- Query: `SELECT net_solar_vs_total_w FROM energy`
- Green fill above 0 (exporting)
- Red fill below 0 (importing)

**Panel 6: Net Solar vs House Only**
- Same as Panel 5 but uses `net_solar_vs_house_only_w`
- Shows if house alone would be grid-independent

**Panel 7: Solar Savings (Bar Chart)**
- Query: Daily aggregation with `time_bucket('1 day', ...)`
- 30-day history
- USD values

**Panel 8: Tesla Fuel Savings (Table + Stat)**
- Query computes gas equivalent vs. electric cost
- Displays breakdown and net savings

### 7.3 Dashboard Variables

```
$electric_rate = 0.16      (Textbox, $/kWh)
$gas_price = 3.80          (Textbox, $/gallon)
$tesla_efficiency = 3.5    (Textbox, miles/kWh)
$ice_mpg = 30.0            (Textbox, MPG)
$interval = 5m             (Dropdown: 1m, 5m, 15m, 1h)
```

Used in queries: `GROUP BY time_bucket($interval, time)`

---

## 8. Deployment

### 8.1 Hardware Requirements

**Minimum:**
- Raspberry Pi 4 (4GB RAM) or equivalent
- 32GB microSD / SSD (for OS + databases)
- Wired Ethernet connection to LAN

**Recommended:**
- NAS with Docker support (Synology, QNAP, TrueNAS)
- Intel N100 mini PC
- 8GB+ RAM for comfortable headroom
- SSD for database storage

### 8.2 Deployment Options

**Option A: Docker Compose (Recommended)**

```yaml
version: '3.8'

services:
  timescaledb:
    image: timescale/timescaledb:latest-pg14
    restart: unless-stopped
    environment:
      POSTGRES_USER: solar
      POSTGRES_PASSWORD: ${DB_PASSWORD}
      POSTGRES_DB: solar
    volumes:
      - timescale-data:/var/lib/postgresql/data
    ports:
      - "5432:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U solar"]
      interval: 10s
      timeout: 5s
      retries: 5

  grafana:
    image: grafana/grafana:latest
    restart: unless-stopped
    environment:
      GF_SECURITY_ADMIN_PASSWORD: ${GRAFANA_PASSWORD}
      GF_INSTALL_PLUGINS: ""
    volumes:
      - grafana-data:/var/lib/grafana
    ports:
      - "3000:3000"
    depends_on:
      - timescaledb

  solar-collector:
    build: ./solar-collector
    restart: unless-stopped
    env_file:
      - .env
    depends_on:
      timescaledb:
        condition: service_healthy

volumes:
  timescale-data:
  grafana-data:
```

**Option B: Systemd Service (Native Binary)**

```ini
# /etc/systemd/system/solar-collector.service
[Unit]
Description=Solar Energy Data Collector
After=network.target postgresql.service
Wants=postgresql.service

[Service]
Type=simple
User=solar
Group=solar
WorkingDirectory=/opt/solar-collector
Environment=RUST_LOG=info
EnvironmentFile=/opt/solar-collector/.env
ExecStart=/opt/solar-collector/solar-collector
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

### 8.3 Installation Steps

1. **Install TimescaleDB:**
   ```bash
   # Using Docker
   docker run -d --name timescaledb \
     -p 5432:5432 \
     -e POSTGRES_PASSWORD=yourpassword \
     -v timescale-data:/var/lib/postgresql/data \
     timescale/timescaledb:latest-pg14
   ```

2. **Initialize Database:**
   ```bash
   psql -h localhost -U postgres -c "CREATE DATABASE solar;"
   psql -h localhost -U postgres -d solar -f schema.sql
   ```

3. **Build Rust Collector:**
   ```bash
   cd solar-collector
   cargo build --release
   strip target/release/solar-collector  # Reduce binary size
   ```

4. **Configure Environment:**
   ```bash
   cp .env.example .env
   nano .env  # Fill in Enphase token, Tesla IP, etc.
   ```

5. **Start Collector:**
   ```bash
   # Docker Compose
   docker-compose up -d
   
   # Or systemd
   sudo systemctl enable solar-collector
   sudo systemctl start solar-collector
   ```

6. **Install Grafana:**
   ```bash
   docker run -d --name grafana \
     -p 3000:3000 \
     -v grafana-data:/var/lib/grafana \
     grafana/grafana:latest
   ```

7. **Configure Grafana Data Source:**
   - Navigate to http://localhost:3000
   - Add PostgreSQL data source
   - Enable TimescaleDB toggle
   - Import dashboard JSON

---

## 9. Monitoring and Maintenance

### 9.1 Health Checks

**Rust Collector:**
- Logs to stdout/stderr (captured by Docker/systemd)
- Emits connection status every 60s: `INFO enphase stream connected` / `WARN enphase stream disconnected, retrying...`

**Database:**
```sql
-- Check last write timestamp
SELECT max(time) FROM energy;

-- Should be within last 10 seconds if running

-- Check row count growth
SELECT count(*) FROM energy WHERE time > now() - interval '1 hour';
-- Should be ~3600 rows/hour at 1/sec
```

**Grafana:**
- Dashboard refresh rate: 5s
- If panels show "No data", check data source connection
- Check Grafana logs: `docker logs grafana`

### 9.2 Backup Strategy

**Database Backups:**
```bash
# Daily backup script
pg_dump -h localhost -U solar -d solar -F c -f /backups/solar_$(date +%Y%m%d).dump

# Restore from backup
pg_restore -h localhost -U solar -d solar /backups/solar_20260221.dump
```

**Grafana Dashboards:**
- Export dashboard JSON regularly
- Store in version control (Git)

### 9.3 Log Rotation

**Systemd journald:**
```bash
# View logs
journalctl -u solar-collector -f

# Limit journal size
sudo journalctl --vacuum-size=100M
```

**Docker logs:**
```yaml
# In docker-compose.yml
services:
  solar-collector:
    logging:
      driver: "json-file"
      options:
        max-size: "10m"
        max-file: "3"
```

---

## 10. Security Considerations

### 10.1 Network Security

- **LAN-only access:** Enphase and Tesla APIs are not exposed to internet
- **No port forwarding:** Grafana accessible only on LAN (or via VPN)
- **TLS for Enphase:** Rust client accepts self-signed cert for `envoy.local`
- **No TLS for Tesla:** Wall Connector uses plain HTTP (acceptable on trusted LAN)

### 10.2 Credential Management

- **Enphase JWT token:** Stored in `.env`, readable only by collector service user
- **Database password:** Stored in `.env` and Docker secrets
- **Grafana admin password:** Set via environment variable, changed from default

### 10.3 Data Privacy

- **No cloud dependencies:** All data stays on local network
- **No external API calls:** System operates offline (after initial token generation)

---

## 11. Future Enhancements

### 11.1 Planned Features

- **MQTT publishing** (optional): Publish metrics to MQTT broker for Home Assistant integration
- **Alerting:** Grafana alerts for:
  - Solar production drop (panel issues)
  - Grid export curtailment
  - Tesla charging anomalies
- **Weather correlation:** Fetch weather API data, store in separate table, JOIN in Grafana queries
- **Peak shaving dashboard:** Identify high-cost time-of-use periods

### 11.2 Possible Optimizations

- **Continuous aggregate on 1-minute intervals:** Reduce query load for real-time panels
- **Compression:** Enable TimescaleDB compression on old data (e.g., >30 days)
- **Partitioning:** Automatic via TimescaleDB hypertables (no manual intervention needed)

---

## 12. Appendix

### 12.1 Glossary

| Term | Definition |
|------|------------|
| **Hypertable** | TimescaleDB's abstraction over chunked time-series data |
| **SSE** | Server-Sent Events — HTTP streaming protocol |
| **DBRP** | Database and Retention Policy (InfluxDB v1 concept) |
| **kWh** | Kilowatt-hour, unit of energy (1000 watts for 1 hour) |
| **Net metering** | Grid export/import tracked by `net-consumption` |
| **CT** | Current Transformer (Enphase uses these to measure consumption) |

### 12.2 References

- **Enphase IQ Gateway Local API:** https://enphase.com/download/iq-gateway-local-apis-or-ui-access-using-token
- **Tesla Wall Connector API (Community):** https://teslamotorsclub.com/tmc/threads/gen3-wall-connector-api.228034/
- **TimescaleDB Documentation:** https://docs.timescale.com/
- **Grafana PostgreSQL Data Source:** https://grafana.com/docs/grafana/latest/datasources/postgres/
- **Tokio Async Runtime:** https://tokio.rs/

### 12.3 Change Log

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2026-02-21 | Initial design document |

---

**End of Document**
