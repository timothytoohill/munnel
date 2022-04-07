#![allow(unused_parens)]

use std::{io};
use tokio;
use log::{info, LevelFilter};
use log4rs::{
    append::{
        console::{ConsoleAppender, Target},
        file::FileAppender,
    },
    config::{Appender, Config, Root},
    encode::pattern::PatternEncoder,
    filter::threshold::ThresholdFilter,
};

mod agent;
mod server;
mod configs;
mod utils;

#[macro_use]
mod constants;

use constants::*;

fn main() -> io::Result<()> {
    let moptions = configs::parse_cmd_line()?;

    if (moptions.mode.len() > 0) {
        if configs::get_log_config_file_path()?.exists() {
            match log4rs::init_file(configs::get_log_config_file_path()?, Default::default()) {
                Err(err) => {
                    println!("Could not initialize log4rs: {}.", err);
                    return Ok(());
                },
                Ok(_) => { }
            }
        } else {
            let level = log::LevelFilter::Info;
            let mut logdir = configs::get_exe_dir()?;
            logdir.push(LOG_FILE);
            let file_path = format!("{}", logdir.display());
            let stdout = ConsoleAppender::builder().encoder(Box::new(PatternEncoder::new("{d} - {l} - {m}{n}"))).target(Target::Stdout).build();
            let logfile = FileAppender::builder().encoder(Box::new(PatternEncoder::new("{d} - {l} - {m}{n}"))).build(file_path).unwrap();
            let config = Config::builder()
                .appender(Appender::builder().build("logfile", Box::new(logfile)))
                .appender(
                    Appender::builder()
                        .filter(Box::new(ThresholdFilter::new(level)))
                        .build("stdout", Box::new(stdout)),
                )
                .build(
                    Root::builder()
                        .appender("logfile")
                        .appender("stdout")
                        .build(LevelFilter::Trace),
                )
                .unwrap();

            match log4rs::init_config(config) {
                Err(err) => {
                    println!("Could not initialize log4rs: {}.", err);
                    return Ok(());
                },
                Ok(_) => { }
            }
        }

        let mode = moptions.mode.as_str();
        match mode {
            "init" => {
                configs::create_config_dir()?;
                configs::init_config_files()?;
                info!("Configs initialized.");
            },
            "agent" => {
                let tokio_run_time = tokio::runtime::Runtime::new()?;
                tokio_run_time.block_on(agent::run_agent(moptions.agent_group_name, moptions.agent_server_endpoint))?;
            },
            "server" => {
                let tokio_run_time = tokio::runtime::Runtime::new()?;
                tokio_run_time.block_on(server::run_server(moptions.server_agent_control_bind_endpoint, moptions.server_configs))?;
            },
            "help" => {
                configs::show_help()?;
            },
            _ => {
                configs::show_syntax_error(format!("Unrecognized mode '{}'.", mode).as_str());
            }
        }
    }

    return Ok(());
}
