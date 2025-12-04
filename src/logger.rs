use std::error::Error;

use log::LevelFilter;
use log4rs::append::file::FileAppender;
use log4rs::{
    append::console::ConsoleAppender,
    config::{Appender, Config as LogConfig, Root},
    encode::pattern::PatternEncoder,
};

use crate::configure::load_config;

pub fn setup_logger() -> Result<(), Box<dyn Error>> {
    let config = load_config()?;
    
    // Parse log level from config
    let log_level = match config.log_level.to_lowercase().as_str() {
        "debug" => LevelFilter::Debug,
        "info" => LevelFilter::Info,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        _ => LevelFilter::Info, // Default to Info
    };

    // Create a stdout appender
    let stdout = ConsoleAppender::builder()
        .encoder(Box::new(PatternEncoder::new(
            "{d(%Y-%m-%d %H:%M:%S)} [{l}] - {m}{n}",
        )))
        .build();

    let mut log_config_builder = LogConfig::builder()
        .appender(Appender::builder().build("stdout", Box::new(stdout)));

    let mut root_builder = Root::builder().appender("stdout");

    // Conditionally add file appender
    if config.log_to_file {
        let log_file = config.log_file;
        println!("Logging to file: {}", log_file);
        
        let file = FileAppender::builder()
            .encoder(Box::new(PatternEncoder::new(
                "{d(%Y-%m-%d %H:%M:%S)} [{l}] - {m}{n}",
            )))
            .build(log_file)?;

        log_config_builder = log_config_builder
            .appender(Appender::builder().build("file", Box::new(file)));
        
        root_builder = root_builder.appender("file");
    }

    let log_config = log_config_builder
        .build(root_builder.build(log_level))?;

    // Initialize the logger
    log4rs::init_config(log_config)?;

    Ok(())
}
