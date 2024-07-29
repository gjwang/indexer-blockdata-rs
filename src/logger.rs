use std::error::Error;

use log4rs::{
    append::console::ConsoleAppender
    ,
    config::{Appender, Config as LogConfig, Root},
    encode::pattern::PatternEncoder,
};
use log4rs::append::file::FileAppender;
use log::LevelFilter;

use crate::configure::load_config;

pub fn setup_logger() -> Result<(), Box<dyn Error>> {
    let config = load_config()?;
    // Get the log file path from the configuration
    let log_file = config.get_string("log_file")?;
    println!("log_file={log_file}");


    // Create a stdout appender
    let stdout = ConsoleAppender::builder()
        .encoder(Box::new(PatternEncoder::new("{d(%Y-%m-%d %H:%M:%S)} [{l}] - {m}{n}")))
        .build();

    // Create a file appender
    let file = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new("{d(%Y-%m-%d %H:%M:%S)} [{l}] - {m}{n}")))
        .build(log_file)?;

    // Build the log4rs configuration
    let log_config = LogConfig::builder()
        .appender(Appender::builder().build("stdout", Box::new(stdout)))
        .appender(Appender::builder().build("file", Box::new(file)))
        .build(
            Root::builder()
                .appender("stdout")
                .appender("file")
                .build(LevelFilter::Info),
        )?;

    // Initialize the logger
    log4rs::init_config(log_config)?;

    Ok(())
}