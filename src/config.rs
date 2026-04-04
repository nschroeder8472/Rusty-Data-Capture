use std::env;

use anyhow::{Context, Result};

#[derive(Clone)]
pub struct Config {
    pub envoy_host: String,
    pub envoy_token: String,
    pub tesla_host: String,
    pub tesla_poll_interval_secs: u64,
    pub database_url: String,
    pub db_pool_size: usize,
    pub write_interval_secs: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        Ok(Self {
            envoy_host: env::var("ENVOY_HOST").context("ENVOY_HOST is required")?,
            envoy_token: env::var("ENVOY_TOKEN").context("ENVOY_TOKEN is required")?,
            tesla_host: env::var("TESLA_HOST").context("TESLA_HOST is required")?,
            tesla_poll_interval_secs: env::var("TESLA_POLL_INTERVAL_SECS")
                .unwrap_or_else(|_| "10".into())
                .parse()
                .context("TESLA_POLL_INTERVAL_SECS must be a number")?,
            database_url: env::var("DATABASE_URL").context("DATABASE_URL is required")?,
            db_pool_size: env::var("DB_POOL_SIZE")
                .unwrap_or_else(|_| "5".into())
                .parse()
                .context("DB_POOL_SIZE must be a number")?,
            write_interval_secs: env::var("WRITE_INTERVAL_SECS")
                .unwrap_or_else(|_| "10".into())
                .parse()
                .context("WRITE_INTERVAL_SECS must be a number")?,
        })
    }
}
