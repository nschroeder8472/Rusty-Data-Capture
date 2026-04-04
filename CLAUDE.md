# CLAUDE.md

## What this is

Real-time solar energy monitor. Rust async service that collects data from an Enphase solar gateway (SSE stream) and a Tesla Wall Connector (HTTP polling), writes to TimescaleDB, visualized in Grafana.

## Architecture

Three independent tokio tasks sharing state via `Arc<Mutex<SharedState>>`:

1. **Enphase streamer** (`src/enphase.rs`) - SSE from `https://{host}/stream/meter`, auto-reconnects
2. **Tesla poller** (`src/tesla.rs`) - HTTP GET `/api/1/vitals` + `/api/1/lifetime` every 10s
3. **DB writer** (`src/database.rs`) - snapshots shared state and inserts to TimescaleDB every 10s

Data flows: Sources -> SharedState -> Writer -> TimescaleDB -> Grafana

## Key files

- `src/main.rs` - entry point, spawns tasks
- `src/config.rs` - env var loading (see `docker/.env.example`)
- `src/metrics.rs` - `EnphaseReading`, `TeslaReading`, `SharedState` structs
- `src/database.rs` - pool, schema creation, inserts, writer loop
- `src/error.rs` - `CollectorError` enum via thiserror
- `schema.sql` - full TimescaleDB DDL (tables, hypertables, aggregates, retention)
- `grafana/dashboard.json` - 8-panel dashboard with cost calculation queries
- `docker/docker-compose.yml` - timescaledb + grafana + solar-collector

## Build & run

```bash
cargo build --release                    # build
cargo test                               # run unit tests
cd docker && docker compose up -d        # full stack (needs .env)
```

## DB schema

Two hypertables: `enphase_readings` and `tesla_readings` with separate sampling rates. Four continuous aggregates (5min + hourly for each). 90-day raw retention. Cost metrics are computed at query time in Grafana, not stored.

## Design doc

`solar-monitor-design.md` is the authoritative spec. Consult it for data source JSON formats, Grafana panel specs, and deployment options.