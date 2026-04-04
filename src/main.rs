mod config;
mod database;
mod enphase;
mod error;
mod metrics;
mod tesla;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::database::{create_pool, ensure_schema, run_writer};
use crate::enphase::run_enphase_stream;
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

    let enphase_handle = tokio::spawn(run_enphase_stream(config.clone(), Arc::clone(&state)));
    let tesla_handle = tokio::spawn(run_tesla_poller(config.clone(), Arc::clone(&state)));
    let writer_handle = tokio::spawn(run_writer(
        pool,
        Arc::clone(&state),
        config.write_interval_secs,
    ));

    info!("All tasks started — collecting data");

    tokio::select! {
        res = enphase_handle => {
            error!("Enphase task exited: {res:?}");
        }
        res = tesla_handle => {
            error!("Tesla task exited: {res:?}");
        }
        res = writer_handle => {
            error!("Writer task exited: {res:?}");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Shutting down...");
        }
    }

    Ok(())
}
