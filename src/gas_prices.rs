use anyhow::{Context, Result};
use chrono::NaiveDate;
use deadpool_postgres::Pool;
use serde::Deserialize;
use tracing::{info, warn};

use crate::config::GasPriceConfig;

const EIA_GAS_PRICE_URL: &str =
    "https://api.eia.gov/v2/petroleum/pri/gnd/data/";

#[derive(Debug, Deserialize)]
struct EiaResponse {
    response: EiaResponseBody,
}

#[derive(Debug, Deserialize)]
struct EiaResponseBody {
    data: Vec<EiaGasRecord>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct EiaGasRecord {
    period: String,
    #[serde(rename = "area-name")]
    area_name: String,
    #[serde(rename = "product-name")]
    product_name: String,
    value: Option<serde_json::Value>,
    units: String,
}

impl EiaGasRecord {
    fn price(&self) -> Option<f64> {
        match &self.value {
            Some(serde_json::Value::Number(n)) => n.as_f64(),
            Some(serde_json::Value::String(s)) => s.parse().ok(),
            _ => None,
        }
    }
}

pub async fn run_gas_price_poller(config: GasPriceConfig, pool: Pool) {
    let interval = tokio::time::Duration::from_secs(config.poll_interval_secs);

    info!(
        "Gas price poller started (every {}s)",
        config.poll_interval_secs
    );

    // Fetch immediately on startup, then on interval
    loop {
        match fetch_and_store(&config, &pool).await {
            Ok(count) => info!("Stored {count} gas price records"),
            Err(e) => warn!("Gas price poll error: {e:#}"),
        }
        tokio::time::sleep(interval).await;
    }
}

async fn fetch_and_store(config: &GasPriceConfig, pool: &Pool) -> Result<usize> {
    let client = reqwest::Client::new();

    let url = format!(
        "{}?api_key={}&frequency=weekly&data[0]=value\
         &facets[product][]=EPMR&facets[duoarea][]=NUS\
         &sort[0][column]=period&sort[0][direction]=desc&length=10",
        EIA_GAS_PRICE_URL, config.eia_api_key
    );

    let resp = client
        .get(&url)
        .send()
        .await
        .context("EIA API request failed")?
        .error_for_status()
        .context("EIA API returned error status")?;

    let eia: EiaResponse = resp.json().await.context("Failed to parse EIA response")?;

    let db = pool.get().await.context("Failed to get DB connection")?;
    let mut count = 0;

    for record in &eia.response.data {
        let price = match record.price() {
            Some(p) => p,
            None => continue,
        };

        let period = NaiveDate::parse_from_str(&record.period, "%Y-%m-%d")
            .context("Failed to parse EIA period date")?;

        db.execute(
            "INSERT INTO gas_prices (period, area_name, product_name, price_per_gallon)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (period, area_name, product_name) DO UPDATE SET price_per_gallon = $4",
            &[&period, &record.area_name, &record.product_name, &price],
        )
        .await
        .context("Failed to insert gas price")?;

        count += 1;
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_eia_response() {
        let json = r#"{
            "response": {
                "total": "1860",
                "dateFormat": "YYYY-MM-DD",
                "frequency": "weekly",
                "data": [
                    {
                        "period": "2026-04-06",
                        "duoarea": "NUS",
                        "area-name": "U.S.",
                        "product": "EPMR",
                        "product-name": "Regular Gasoline",
                        "process": "PTE",
                        "process-name": "Retail Sales",
                        "series": "EMM_EPMR_PTE_NUS_DPG",
                        "series-description": "U.S. Regular All Formulations Retail Gasoline Prices",
                        "value": "4.12",
                        "units": "$/GAL"
                    }
                ]
            }
        }"#;

        let eia: EiaResponse = serde_json::from_str(json).unwrap();
        assert_eq!(eia.response.data.len(), 1);
        let record = &eia.response.data[0];
        assert_eq!(record.area_name, "U.S.");
        assert_eq!(record.product_name, "Regular Gasoline");
        assert!((record.price().unwrap() - 4.12).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_numeric_value() {
        let json = r#"{
            "response": {
                "data": [
                    {
                        "period": "2026-04-06",
                        "area-name": "U.S.",
                        "product-name": "Regular Gasoline",
                        "value": 3.99,
                        "units": "$/GAL"
                    }
                ]
            }
        }"#;

        let eia: EiaResponse = serde_json::from_str(json).unwrap();
        assert!((eia.response.data[0].price().unwrap() - 3.99).abs() < f64::EPSILON);
    }

    #[test]
    fn test_null_value_skipped() {
        let json = r#"{
            "response": {
                "data": [
                    {
                        "period": "2026-04-06",
                        "area-name": "U.S.",
                        "product-name": "Regular Gasoline",
                        "value": null,
                        "units": "$/GAL"
                    }
                ]
            }
        }"#;

        let eia: EiaResponse = serde_json::from_str(json).unwrap();
        assert!(eia.response.data[0].price().is_none());
    }
}
