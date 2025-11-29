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
}

pub fn load_config() -> Result<AppConfig, ConfigError> {
    let s = Config::builder()
        // Set defaults
        .set_default("log_file", "log/eth_block_data.log")?
        .set_default("centrifugo_url", "ws://localhost:8000/connection/websocket")?
        .set_default("centrifugo_channel", "news")?
        .set_default("kafka_broker", "localhost:9093")?
        .set_default("kafka_topic", "latency-test-topic")?
        .set_default("kafka_group_id", "centrifugo-pusher")?
        // Add configuration from a file
        .add_source(File::with_name("config/config.yaml"))
        // Add configuration from environment variables
        .add_source(config::Environment::with_prefix("APP"))
        .build()?;

    s.try_deserialize()
}
