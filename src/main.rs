mod config;
mod database;
mod enphase;
mod error;
mod gas_prices;
mod metrics;
mod tesla;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::database::{create_pool, ensure_schema, run_writer};
use crate::enphase::run_enphase_stream;
use crate::gas_prices::run_gas_price_poller;
use crate::metrics::SharedState;
use crate::tesla::run_tesla_poller;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = Config::from_env()?;
    info!("Configuration loaded");

    let pool = create_pool(&config)?;
    ensure_schema(&pool).await?;

    let state = Arc::new(Mutex::new(SharedState::default()));

    let mut handles: Vec<(&str, tokio::task::JoinHandle<()>)> = Vec::new();

    if let Some(enphase_config) = config.enphase.clone() {
        info!("Enphase collector enabled");
        handles.push((
            "Enphase",
            tokio::spawn(run_enphase_stream(enphase_config, Arc::clone(&state))),
        ));
    } else {
        warn!("Enphase collector disabled (ENVOY_HOST / ENVOY_TOKEN not set)");
    }

    if let Some(tesla_config) = config.tesla.clone() {
        info!("Tesla collector enabled");
        handles.push((
            "Tesla",
            tokio::spawn(run_tesla_poller(tesla_config, Arc::clone(&state))),
        ));
    } else {
        warn!("Tesla collector disabled (TESLA_HOST not set)");
    }

    if let Some(gas_config) = config.gas_prices.clone() {
        info!("Gas price collector enabled");
        handles.push((
            "Gas prices",
            tokio::spawn(run_gas_price_poller(gas_config, pool.clone())),
        ));
    } else {
        warn!("Gas price collector disabled (EIA_API_KEY not set)");
    }

    let has_enphase = config.enphase.is_some();
    let has_tesla = config.tesla.is_some();

    if has_enphase || has_tesla {
        handles.push((
            "Writer",
            tokio::spawn(run_writer(
                pool,
                Arc::clone(&state),
                config.write_interval_secs,
                has_enphase,
                has_tesla,
            )),
        ));
    }

    if handles.is_empty() {
        warn!("No data sources enabled — nothing to do");
        return Ok(());
    }

    info!("All tasks started — collecting data");

    tokio::select! {
        result = async {
            loop {
                for (name, handle) in &mut handles {
                    if handle.is_finished() {
                        return *name;
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        } => {
            error!("{result} task exited unexpectedly");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Shutting down...");
        }
    }

    Ok(())
}
