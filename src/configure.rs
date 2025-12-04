#![allow(dead_code)]
use config::{Config, ConfigError, File};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct KafkaTopics {
    pub orders: String,
    pub trades: String,
    pub balance_ops: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct KafkaConfig {
    pub broker: String,
    pub topics: KafkaTopics,
    pub group_id: String,
    // Kafka Tuning
    pub linger_ms: String,
    pub fetch_wait_max_ms: String,
    pub session_timeout_ms: String,
    pub heartbeat_interval_ms: String,
    pub max_poll_interval_ms: String,
    pub socket_keepalive_enable: String,
}

#[derive(Debug, Deserialize)]
pub struct CentrifugoConfig {
    pub url: String,
    pub channel: String,
}

#[derive(Debug, Deserialize)]
pub struct ZmqConfig {
    pub settlement_port: u16,
    pub market_data_port: u16,
}

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub log_file: String,
    pub centrifugo: CentrifugoConfig,
    pub kafka: KafkaConfig,
    #[serde(default)]
    pub enable_local_wal: bool,
    pub zeromq: Option<ZmqConfig>,
}

pub fn load_config() -> Result<AppConfig, ConfigError> {
    let run_mode = std::env::var("RUN_MODE").unwrap_or_else(|_| "dev".into());
    let config_file_name = format!("config/{}", run_mode);

    let s = Config::builder()
        // 1. Start with "default" configuration
        .add_source(File::with_name("config/config"))
        // 2. Add in the current environment file (optional)
        .add_source(File::with_name(&config_file_name).required(false))
        // 3. Add in settings from Environment Variables
        //    E.g. APP__KAFKA__BROKER=...
        .add_source(config::Environment::with_prefix("APP").separator("__"))
        .build()?;

    s.try_deserialize()
}
