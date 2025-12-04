use std::error::Error;
use std::fs;
use std::path::Path;

use log::LevelFilter;
use log4rs::append::file::FileAppender;
use log4rs::{
    append::console::ConsoleAppender,
    config::{Appender, Config as LogConfig, Root},
    encode::pattern::PatternEncoder,
};

use crate::configure::load_config;

/// Standard log pattern with timestamp, level, target, and message
const LOG_PATTERN: &str = "{d(%Y-%m-%d %H:%M:%S%.3f)} [{l}] [{t}] - {m}{n}";

/// Parse log level from string, supporting RUST_LOG environment variable override
fn parse_log_level(level_str: &str) -> LevelFilter {
    // Check RUST_LOG environment variable first (standard Rust convention)
    if let Ok(rust_log) = std::env::var("RUST_LOG") {
        return match rust_log.to_lowercase().as_str() {
            "trace" => LevelFilter::Trace,
            "debug" => LevelFilter::Debug,
            "info" => LevelFilter::Info,
            "warn" => LevelFilter::Warn,
            "error" => LevelFilter::Error,
            "off" => LevelFilter::Off,
            _ => {
                eprintln!("Invalid RUST_LOG value: {}, defaulting to Info", rust_log);
                LevelFilter::Info
            }
        };
    }

    // Fall back to config value
    match level_str.to_lowercase().as_str() {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "info" => LevelFilter::Info,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        "off" => LevelFilter::Off,
        _ => {
            eprintln!("Invalid log_level in config: {}, defaulting to Info", level_str);
            LevelFilter::Info
        }
    }
}

/// Setup the logger with console and optional file output
///
/// # Configuration
/// - Respects `RUST_LOG` environment variable (standard Rust convention)
/// - Falls back to `log_level` in config
/// - Supports levels: trace, debug, info, warn, error, off
/// - Conditionally logs to file based on `log_to_file` config
///
/// # Errors
/// Returns error if:
/// - Config cannot be loaded
/// - Log file directory cannot be created
/// - Log4rs initialization fails
pub fn setup_logger() -> Result<(), Box<dyn Error>> {
    let config = load_config()?;
    
    let log_level = parse_log_level(&config.log_level);

    // Create console appender with colored output
    let stdout = ConsoleAppender::builder()
        .encoder(Box::new(PatternEncoder::new(LOG_PATTERN)))
        .build();

    let mut log_config_builder = LogConfig::builder()
        .appender(Appender::builder().build("stdout", Box::new(stdout)));

    let mut root_builder = Root::builder().appender("stdout");

    // Conditionally add file appender
    if config.log_to_file {
        let log_file = &config.log_file;
        
        // Ensure log directory exists
        if let Some(parent) = Path::new(log_file).parent() {
            fs::create_dir_all(parent)?;
        }
        
        eprintln!("Logging to file: {} (level: {:?})", log_file, log_level);
        
        let file = FileAppender::builder()
            .encoder(Box::new(PatternEncoder::new(LOG_PATTERN)))
            .build(log_file)?;

        log_config_builder = log_config_builder
            .appender(Appender::builder().build("file", Box::new(file)));
        
        root_builder = root_builder.appender("file");
    } else {
        eprintln!("Logging to console only (level: {:?})", log_level);
    }

    let log_config = log_config_builder
        .build(root_builder.build(log_level))?;

    // Initialize the logger
    log4rs::init_config(log_config)?;

    Ok(())
}
