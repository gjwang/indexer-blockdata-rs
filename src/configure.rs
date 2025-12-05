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

#[derive(Debug, Deserialize, Clone)]
pub struct ScyllaDbConfig {
    pub hosts: Vec<String>,
    pub keyspace: String,
    pub replication_factor: u32,
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_ms: u64,
    #[serde(default = "default_request_timeout")]
    pub request_timeout_ms: u64,
}

fn default_connection_timeout() -> u64 {
    5000
}

fn default_request_timeout() -> u64 {
    3000
}

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub log_file: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_log_to_file")]
    pub log_to_file: bool,
    pub centrifugo: CentrifugoConfig,
    pub kafka: KafkaConfig,
    #[serde(default)]
    pub enable_local_wal: bool,
    pub zeromq: Option<ZmqConfig>,
    pub scylladb: Option<ScyllaDbConfig>,
    // Settlement service configuration
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
    #[serde(default)]
    pub backup_csv_file: String,
    #[serde(default)]
    pub failed_trades_file: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_to_file() -> bool {
    true
}

fn default_data_dir() -> String {
    "./data".to_string()
}

/// Expand tilde (~) in path to home directory
///
/// # Examples
/// ```
/// use fetcher::configure::expand_tilde;
/// let path = expand_tilde("~/data");
/// // Returns: "/home/user/data" (or "/Users/user/data" on macOS)
/// ```
pub fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            if let Some(home_str) = home.to_str() {
                return path.replacen("~", home_str, 1);
            }
        }
    }
    path.to_string()
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

/// Load configuration for a specific service with a custom config file
///
/// # Arguments
/// * `config_file` - Name of the service config file (e.g., "settlement_config", "matching_engine_config")
///                   The `.yaml` extension is added automatically.
///
/// # Configuration Loading Order (later sources override earlier ones):
/// 1. `config/config.yaml` - Base configuration
/// 2. `config/{RUN_MODE}.yaml` - Environment-specific config (dev/prod/test)
/// 3. `config/{config_file}.yaml` - Service-specific config (highest priority)
/// 4. Environment variables with `APP__` prefix
///
/// # Example
/// ```no_run
/// use fetcher::configure::load_service_config;
/// // Loads: config.yaml -> dev.yaml -> settlement_config.yaml -> env vars
/// let config = load_service_config("settlement_config")?;
/// # Ok::<(), config::ConfigError>(())
/// ```
///
/// This allows each service to have isolated configuration without affecting others.
pub fn load_service_config(config_file: &str) -> Result<AppConfig, ConfigError> {
    let run_mode = std::env::var("RUN_MODE").unwrap_or_else(|_| "dev".into());
    let env_config_file = format!("config/{}", run_mode);
    let service_config_file = format!("config/{}", config_file);

    let s = Config::builder()
        // 1. Start with "default" configuration
        .add_source(File::with_name("config/config"))
        // 2. Add in the current environment file (optional)
        .add_source(File::with_name(&env_config_file).required(false))
        // 3. Add service-specific config (highest priority for file-based config)
        .add_source(File::with_name(&service_config_file).required(false))
        // 4. Add in settings from Environment Variables (ultimate override)
        //    E.g. APP__KAFKA__BROKER=...
        .add_source(config::Environment::with_prefix("APP").separator("__"))
        .build()?;

    s.try_deserialize()
}
