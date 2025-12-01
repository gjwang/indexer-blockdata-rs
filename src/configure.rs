#![allow(dead_code)]
use config::{Config, ConfigError, File};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub log_file: String,
    pub centrifugo_url: String,
    pub centrifugo_channel: String,
    pub kafka_broker: String,
    pub kafka_topic: String,
    pub kafka_group_id: String,
    // Kafka Tuning
    pub kafka_linger_ms: String,
    pub kafka_fetch_wait_max_ms: String,
    pub kafka_session_timeout_ms: String,
    pub kafka_heartbeat_interval_ms: String,
    pub kafka_max_poll_interval_ms: String,
    pub kafka_socket_keepalive_enable: String,
}

pub fn load_config() -> Result<AppConfig, ConfigError> {
    let run_mode = std::env::var("RUN_MODE").unwrap_or_else(|_| "dev".into());
    let config_file_name = format!("config/{}.yaml", run_mode);

    let s = Config::builder()
        // Set defaults
        .set_default("log_file", "log/eth_block_data.log")?
        .set_default("centrifugo_url", "ws://localhost:8000/connection/websocket")?
        .set_default("centrifugo_channel", "news")?
        .set_default("kafka_broker", "localhost:9093")?
        .set_default("kafka_topic", "orders")?
        .set_default("kafka_group_id", "matching_engine_group")?
        // Kafka Tuning Defaults
        .set_default("kafka_linger_ms", "3")?
        .set_default("kafka_fetch_wait_max_ms", "3")?
        .set_default("kafka_session_timeout_ms", "10000")?
        .set_default("kafka_heartbeat_interval_ms", "3000")?
        .set_default("kafka_max_poll_interval_ms", "30000")?
        .set_default("kafka_socket_keepalive_enable", "true")?
        // Add configuration from a file
        .add_source(File::with_name(&config_file_name).required(false))
        // Add configuration from environment variables
        .add_source(config::Environment::with_prefix("APP"))
        .build()?;

    s.try_deserialize()
}
