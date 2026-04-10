use std::env;

use anyhow::{Context, Result};

#[derive(Clone)]
pub struct EnphaseConfig {
    pub host: String,
    pub token: String,
}

#[derive(Clone)]
pub struct TeslaConfig {
    pub host: String,
    pub poll_interval_secs: u64,
}

#[derive(Clone)]
pub struct GasPriceConfig {
    pub eia_api_key: String,
    pub poll_interval_secs: u64,
}

#[derive(Clone)]
pub struct Config {
    pub enphase: Option<EnphaseConfig>,
    pub tesla: Option<TeslaConfig>,
    pub gas_prices: Option<GasPriceConfig>,
    pub database_url: String,
    pub db_pool_size: usize,
    pub write_interval_secs: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        let enphase = match (env::var("ENVOY_HOST"), env::var("ENVOY_TOKEN")) {
            (Ok(host), Ok(token)) => Some(EnphaseConfig { host, token }),
            _ => None,
        };

        let tesla = env::var("TESLA_HOST").ok().map(|host| {
            let poll_interval_secs = env::var("TESLA_POLL_INTERVAL_SECS")
                .unwrap_or_else(|_| "10".into())
                .parse()
                .unwrap_or(10);
            TeslaConfig {
                host,
                poll_interval_secs,
            }
        });

        let gas_prices = env::var("EIA_API_KEY").ok().map(|eia_api_key| {
            let poll_interval_secs = env::var("GAS_PRICE_POLL_INTERVAL_SECS")
                .unwrap_or_else(|_| "86400".into())
                .parse()
                .unwrap_or(86400);
            GasPriceConfig {
                eia_api_key,
                poll_interval_secs,
            }
        });

        Ok(Self {
            enphase,
            tesla,
            gas_prices,
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
