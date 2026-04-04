# Rusty-Data-Capture

A real-time solar energy monitoring system built in Rust. Collects data from an Enphase IQ Gateway and Tesla Wall Connector, stores it in TimescaleDB, and visualizes it in Grafana with cost savings calculations.

## Features

- **Enphase solar monitoring** — SSE stream capturing solar production, house consumption, and grid import/export at ~1s resolution
- **Tesla Wall Connector tracking** — HTTP polling for charging power, session energy, and lifetime stats
- **TimescaleDB storage** — hypertables with automatic 5-minute and hourly continuous aggregates, 90-day raw data retention
- **Grafana dashboard** — 8 pre-built panels including real-time power flow, net solar balance, and cost savings analysis
- **Lightweight** — async Rust binary suitable for Raspberry Pi or NAS deployment

## Architecture

```
Enphase IQ Gateway (SSE ~1/sec) ──┐
                                   ├── SharedState (Arc<Mutex>) ── Writer (10s) ── TimescaleDB ── Grafana
Tesla Wall Connector (HTTP ~10s) ─┘
```

Three independent tokio tasks collect and write data. Derived metrics (house load excluding Tesla, cost savings) are computed at query time in Grafana.

## Quick Start

### Prerequisites

- Docker & Docker Compose
- Enphase IQ Gateway on your LAN with a [local API JWT token](https://enphase.com/download/iq-gateway-local-apis-or-ui-access-using-token)
- Tesla Wall Connector Gen 3 on your LAN

### Setup

1. **Configure environment:**
   ```bash
   cp docker/.env.example docker/.env
   # Edit docker/.env with your Enphase token, Tesla IP, and passwords
   ```

2. **Start the stack:**
   ```bash
   cd docker
   docker compose up -d
   ```

3. **Open Grafana** at `http://localhost:3000` (login with admin / your `GRAFANA_PASSWORD`).

The dashboard and TimescaleDB datasource are auto-provisioned on first start.

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `ENVOY_HOST` | — | Enphase gateway hostname (e.g. `envoy.local`) |
| `ENVOY_TOKEN` | — | JWT token for Enphase local API |
| `TESLA_HOST` | — | Tesla Wall Connector IP address |
| `TESLA_POLL_INTERVAL_SECS` | `10` | Tesla polling interval |
| `WRITE_INTERVAL_SECS` | `10` | How often to write snapshots to the database |
| `DB_PASSWORD` | — | Shared password for TimescaleDB and the collector |
| `DB_POOL_SIZE` | `5` | Database connection pool size |
| `GRAFANA_PASSWORD` | — | Grafana admin password |
| `RUST_LOG` | `info` | Log level (`debug`, `info`, `warn`, `error`) |

Cost calculation parameters (electric rate, gas price, Tesla efficiency, ICE MPG) are configured as Grafana dashboard variables — no restart required to adjust them.

## Building from Source

```bash
cargo build --release
cargo test
```

Requires Rust 1.85+. The binary expects environment variables or a `.env` file (loaded via dotenvy).

## Dashboard

The Grafana dashboard includes:

| Panel | Description |
|-------|-------------|
| Solar Generation | Current solar output with sparkline |
| House Load | Total house consumption |
| Tesla Charging | Current charging power |
| Power Flow | Time series of solar, house, grid, and Tesla |
| Net Solar vs Total | Surplus/deficit including Tesla charging |
| Net Solar vs House Only | Surplus/deficit excluding Tesla |
| Solar Savings | Daily dollar savings from solar offset (30 days) |
| Tesla Fuel Savings | EV vs gasoline cost comparison table (30 days) |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## License

[MIT](LICENSE)
