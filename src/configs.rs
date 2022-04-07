use std::{env, path, io, fs, collections::HashMap, io::prelude::*};
use log::{info, warn};

use crate::config_dir;
use crate::constants::*;
use crate::utils::*;

#[derive(Clone, Debug, Default)]
pub struct ServerConfig {
    pub service_name:String,
    pub agent_group_name:String,
    pub server_bind_endpoint:String,
    pub agent_service_endpoint:String
}

pub type ServerConfigs = HashMap<String, ServerConfig>;

pub struct AppOptions {
    pub mode:String,
    pub agent_server_endpoint:String,
    pub agent_group_name:String,
    pub server_agent_control_bind_endpoint:String,
    pub server_configs:ServerConfigs
}

pub fn parse_cmd_line() -> io::Result<AppOptions> {
    let args:Vec<String> = env::args().collect();
    let mut mode = String::from("");
    let mut agent_server_endpoint = String::from("");
    let mut agent_group_name = String::from("");
    let mut server_agent_control_bind_endpoint = String::from(DEFAULT_AGENT_CONTROL_BIND_ENDPOINT);
    let mut server_configs = ServerConfigs::new();

    if (args.len() > 1) {
        mode = args[1].to_lowercase();
        match mode.as_str() {
            "agent" => {
                if (args.len() > 2) {
                    agent_server_endpoint = args[2].clone();
                    if (args.len() > 3) {
                        agent_group_name = args[3].clone();
                    }
                } else {
                    show_syntax_error("The server endpoint to which the agent connects must be specified.");
                }
            },
            "server" => {
                if (args.len() > 2) {
                    let mut index = 2;
                    if (args[index].contains(" ")) { } else {
                        server_agent_control_bind_endpoint = args[index].clone();
                        index = index + 1;
                    }
                    while (index < args.len()) {
                        let sconfig = parse_server_config_line(&args[index])?;
                        add_server_config(&sconfig, &mut server_configs);
                        index = index + 1;
                    }
                }
            },
            _ => {

            }
        }        
    } else {
        show_syntax_error("The mode must be specified.");
    }

    let moptions = AppOptions {mode, agent_server_endpoint, agent_group_name, server_agent_control_bind_endpoint, server_configs};

    return Ok(moptions);
}

pub fn show_syntax_error(error:&str) {
    let exe_name = APP_NAME.to_lowercase();

    println!("Syntax error: {}", error);
    println!("For help, run: {} help", exe_name);
}

pub fn show_help() -> io::Result<()> {
    let exe_name = APP_NAME.to_lowercase();

    println!("{} runs in agent or server mode.", APP_NAME);
    println!("Server mode optionally reads the '{}' file for service configs. Agent mode does not require a config file.", get_config_file_path()?.display());
    println!("{} reads the '{}' file for logging configs, but it is not necessary.", APP_NAME, get_log_config_file_path()?.display());
    println!("Note that it is not necessary to run the init command to use {}.", APP_NAME);
    println!("");
    println!("Values between < and > are required. Values between [ and ] are optional. The default server bind address to which agents connect is {}.", DEFAULT_AGENT_CONTROL_BIND_ENDPOINT);
    println!("");
    println!("To initialize configs:");
    println!("\t{} init", exe_name);
    println!("To run in agent mode:");
    println!("\t{} agent <SERVER_ADDRESS:PORT_NUMBER> [GROUP_NAME]", exe_name);
    println!("To run in server mode:");
    println!("\t{} server [LOCAL_BIND_ADDRESS:PORT_NUMBER] [SERVICE_CONFIGS] [SERVICE_CONFIGS] [...]", exe_name);
    println!("");
    println!("If a group name is not specfied for an agent, then it is considered available to handle all service connections.");
    println!("");
    println!("Service configs in the config file have the same syntax as the command line:");
    println!("\t<SERVICE_NAME> <AGENT_GROUP_NAME> <SERVER_BIND_ADDRESS:PORT_NUMBER> <AGENT_DEST_ADDRESS:PORT_NUMBER>");
    println!("On the command line, the service configs have to be enclosed in quotes (\").");
    println!("");
    println!("For additional help or information, go to https://github.com/timothytoohill/munnel.");

    return Ok(());
}


pub fn get_exe_dir() -> io::Result<path::PathBuf> {
    let mut dir = env::current_exe()?;
    dir.pop();
    return Ok(dir);
}

pub fn get_config_path() -> io::Result<path::PathBuf> {
    let mut _dir = get_exe_dir()?;
    let mut dir = env::current_dir()?;
    dir.push(config_dir!());
    return Ok(dir);
}


pub fn get_config_file_path() -> io::Result<path::PathBuf> {
    let mut dir = get_config_path()?;
    dir.push(CONFIG_FILE);
    return Ok(dir);
}

pub fn get_log_config_file_path() -> io::Result<path::PathBuf> {
    let mut dir = get_config_path()?;
    dir.push(LOG_CONFIG_FILE);
    return Ok(dir);
}

pub fn create_config_dir() -> io::Result<()> {
    let config_path = get_config_path()?;
    if config_path.exists() {

    } else {
        fs::create_dir(config_path)?;
    }
    return Ok(());
}

pub fn init_config_files() -> io::Result<()> {
    let log_config = 
r#"
refresh_rate: 30 seconds
appenders:
  stdout:
    kind: console
    encoder:
      pattern: "{d} - {m}{n}"
  file:
    kind: file
    path: "log/requests.log"
    encoder:
      pattern: "{d} - {m}{n}"
root:
  level: info
  appenders:
    - stdout
    - file
"#;
    let log_config_path = get_log_config_file_path()?;

    if log_config_path.exists() {
    } else {
        println!("Creating log4rs config file '{}'...", log_config_path.display());
        fs::write(&log_config_path, log_config)?;
        println!("Done creating log4rs config file '{}'.", log_config_path.display());
    }
    return Ok(());
}

pub fn load_server_configs() -> io::Result<ServerConfigs> {
    let conf_path = get_config_file_path()?;
    let mut configs = ServerConfigs::new();
    if (conf_path.exists()) {
        let conf_file = fs::File::open(conf_path)?;
        let mut reader = io::BufReader::new(conf_file);
        let mut line = String::new();

        while (reader.read_line(&mut line).unwrap() > 0) {
            line = rm_newline(line);
            let sconfig = parse_server_config_line(&line)?;
            add_server_config(&sconfig, &mut configs);
            line = String::new();
        }
    } else {
        info!("The '{}' config file was not found.", conf_path.display());
    }
    return Ok(configs);
}

pub fn parse_server_config_line(line:&String) -> io::Result<ServerConfig> {
    let params:Vec<&str> = line.split(' ').collect();
    let service_name = String::from(params[0]);
    let agent_group_name = String::from(params[1]);
    let server_bind_endpoint = String::from(params[2]);
    let agent_service_endpoint = String::from(params[3]);
    let sconfig = ServerConfig { service_name, agent_group_name, server_bind_endpoint, agent_service_endpoint };
    return Ok(sconfig);
}

pub fn add_server_config(sconfig:&ServerConfig, sconfigs:&mut ServerConfigs) {
    let service_name = sconfig.service_name.clone();
    let group_name = sconfig.agent_group_name.clone();
    let key = get_server_config_key(&service_name, &group_name);
    if (sconfigs.contains_key(&key)) {
        warn!("Duplicate server config is ignored - Service: {}, Group: {}.", service_name, group_name);
    } else {
        sconfigs.insert(key, sconfig.clone());
    }
}

pub fn get_server_config_key(service_name:&String, group_name:&String) -> String {
    return String::from(service_name.to_owned() + ":" + group_name.as_str());
}