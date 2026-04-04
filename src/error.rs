use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum CollectorError {
    #[error("Enphase connection failed: {0}")]
    EnphaseConnection(#[from] reqwest::Error),

    #[error("Enphase SSE parse error: {0}")]
    EnphaseParse(String),

    #[error("Tesla request failed: {0}")]
    TeslaRequest(String),

    #[error("Tesla parse error: {0}")]
    TeslaParse(String),

    #[error("Database error: {0}")]
    Database(#[from] tokio_postgres::Error),

    #[error("Pool error: {0}")]
    Pool(String),

    #[error("Config error: {0}")]
    Config(String),
}
