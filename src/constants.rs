#[macro_export]
macro_rules! config_dir {
    () => ("") 
}

pub const APP_NAME:&str = "Munnel";
pub const CONFIG_FILE:&str = concat!(config_dir!(), "munnel", ".ini");
pub const LOG_FILE:&str = "munnel.log";
pub const LOG_CONFIG_FILE:&str = concat!(config_dir!(), "log4rs.yaml");
pub const DEFAULT_AGENT_CONTROL_BIND_ENDPOINT:&str = "0.0.0.0:10000";

pub const CMD_NEW_AGENT:&str = "NEW_AGENT";
pub const CMD_PSK:&str = "PRE_SHARED_KEY";
pub const CMD_CONNECT:&str = "CONNECT";
pub const CMD_OK:&str = "OK";
pub const CMD_KEEP_ALIVE:&str = "KEEP_ALIVE";
pub const CMD_DEREGISTER:&str = "DEREGISTER";
pub const CMD_SHUTDOWN:&str = "SHUTDOWN";
pub const CMD_NEW_SERVICE:&str = "NEW_SERVICE";
pub const CMD_CANCEL_CONNECTION:&str = "CANCEL_CONNECTION";

pub const WAIT_RECONNECT_MS:u64 = 5000;
pub const KEEP_ALIVE_INTERVAL_MS:u64 = 5000;
pub const RECONCILE_INTERVAL_MS:u64 = 1000;

pub const DEFAULT_PRE_SHARED_KEY:&str = "ABC123";
pub const MAX_AGENT_CONNECTIONS:usize = 1000; //added for security. Should be made configurable