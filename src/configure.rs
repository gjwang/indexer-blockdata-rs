use config::{Config, ConfigError, File};

pub fn load_config() -> Result<Config, ConfigError> {
    Config::builder()
        // Set defaults
        // .set_default("log_file", "log/default.log")?
        // Add configuration from a file
        .add_source(File::with_name("config/config.yaml"))
        // Add configuration from environment variables
        .add_source(config::Environment::with_prefix("APP"))
        .build()
}