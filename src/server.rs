use std::{collections::HashMap, net::SocketAddr};
use tokio::{io, net::{TcpListener, TcpStream}, sync::{mpsc, Mutex}, time::{interval, Duration}};
use log::{error, info, warn};

use crate::configs::*;
use crate::constants::*;
use crate::utils::*;

#[derive(Debug, Default)]
struct AppCommand {
    pub id:String,
    pub name:String,
    pub tx:Option<mpsc::Sender<AppCommand>>,
    pub group_name:Option<String>,
    pub service_name:Option<String>,
    pub from_address:Option<SocketAddr>,
    pub connection_id:Option<String>,
    pub stream:Option<SharedTokioStream>,
    pub agent_dest_address:Option<String>,
    pub counter:Option<usize>
}

pub type SharedTokioStream = std::sync::Arc<tokio::sync::Mutex<tokio::net::TcpStream>>;

type AgentControlThreads = HashMap<String, AppCommand>;
type AgentControlThreadsByGroupName = HashMap<String, AgentControlThreads>;
type ServiceControlThreadsByServiceName = HashMap<String, AppCommand>;
type PendingConnections = HashMap<String, AppCommand>;

/*
The main server event loop. This is where most of the state is housed and maintained, such as pending connections,
service-to-agent mappings, etc. Async threads are spawned from here to handle listening sockets and streaming between
an agent and the client.
*/
pub async fn run_server(server_agent_control_bind_endpoint:String, cmd_line_server_configs:ServerConfigs, pre_shared_key:String) -> io::Result<()> {
    let endpoint = server_agent_control_bind_endpoint;
  
    //instantiations of state that is used to manage and map agents, connections, and services. channels are used to coordinate state among threads
    let server_service_configs = load_server_services(cmd_line_server_configs.clone())?;
    let mut agent_control_threads_by_group_name = AgentControlThreadsByGroupName::new();
    let mut service_control_threads_by_service_name = ServiceControlThreadsByServiceName::new();
    let mut pending_connections_by_id = PendingConnections::new();

    //use of message passing for synchronization between this main async thread and spawned child async threads
    let (act_tx, mut act_rx) = mpsc::channel::<AppCommand>(1000);
    let (sct_tx, mut sct_rx) = mpsc::channel::<AppCommand>(1000);

    //for now, exit when no configs, but eventually, implement hot reload
    if (server_service_configs.len() <= 0) { return Ok(()); }

    //listen on agent control port for new agents.
    match TcpListener::bind(&endpoint).await {
        Err(e) => {
            error!("Could not listen on {}. Error: {}", endpoint, e);
        },
        Ok(listener) => {
            info!("Running in server mode. Listening for agent control connections on: {}.", endpoint);

            let mut reconcile_timer = interval(Duration::from_millis(RECONCILE_INTERVAL_MS));

            //main event loop
            loop {
                let listener_accept = listener.accept();
                let act_recv = act_rx.recv();
                let sct_recv = sct_rx.recv();
                let reconcile = reconcile_timer.tick();

                tokio::select!(
                    _result = reconcile => {
                        reconcile_agents_and_services(server_service_configs.clone(), &agent_control_threads_by_group_name, &mut service_control_threads_by_service_name, sct_tx.clone());
                    },
                    result = listener_accept => {
                        match result {
                            Err(e) => {
                                error!("Error accepting connection: {}.", e);
                                break;
                            },
                            Ok((stream, address)) => {
                                info!("Accepted connection on agent control port from: {}.", address);
                                if (get_connected_agent_count(&agent_control_threads_by_group_name) >= MAX_AGENT_CONNECTIONS) {
                                    error!("Closing new agent contorl connection from: {}. Exceeded MAX_AGENT_CONNECTIONS.", address);
                                } else {
                                    let new_act_tx = act_tx.clone();
                                    let id = get_rand_guid();
                                    let psk = pre_shared_key.clone();
                                    tokio::spawn(async move { 
                                        agent_control_thread(id.clone(), stream, new_act_tx.clone(), address, psk).await;

                                        //deregister
                                        let mcmd = AppCommand{ id: id.clone(), name: String::from(CMD_DEREGISTER), from_address: Some(address), ..AppCommand::default() };
                                        match new_act_tx.send(mcmd).await {
                                            Err(e) => {
                                                error!("Could not send deregister command: {}.", e);
                                            },
                                            Ok(_result) => { }
                                        }
                                    });
                                }
                            }
                        }
                    },
                    act_option = act_recv => {
                        let mcmd = act_option.unwrap();
                        let cmd = mcmd.name.clone();
                        let agent_thread_id = mcmd.id.clone();
                        let address = mcmd.from_address.clone().unwrap();
                        match cmd.as_str() {
                            CMD_NEW_AGENT => {
                                let group_name = mcmd.group_name.clone().unwrap();

                                if (group_name.len() > 0) {
                                    info!("Agent connected ({}) that belongs to the {} group.", address, group_name);
                                } else {
                                    info!("Agent connected ({}) that belongs to all groups.", address);
                                }
                                
                                if (agent_control_threads_by_group_name.contains_key(&group_name)) { 
                                    //already has group 
                                } else {
                                    agent_control_threads_by_group_name.insert(group_name.clone(), AgentControlThreads::new());
                                }
                                agent_control_threads_by_group_name.get_mut(&group_name).unwrap().insert(agent_thread_id, mcmd);
                                info!("Registered agent ({}) to handle connections.", address);

                                reconcile_agents_and_services(server_service_configs.clone(), &agent_control_threads_by_group_name, &mut service_control_threads_by_service_name, sct_tx.clone());
                            },
                            CMD_CONNECT => {
                                let connection_id = mcmd.connection_id.clone().unwrap();
                                if (pending_connections_by_id.contains_key(&connection_id)) {
                                    let agent_stream = mcmd.stream.unwrap();
                                    let agent_from_address = mcmd.from_address.clone().unwrap();
                                    let (_conn_id, pending_mcmd) = pending_connections_by_id.remove_entry(&connection_id).unwrap();
                                    let server_stream = pending_mcmd.stream.unwrap();
                                    let server_from_address = pending_mcmd.from_address.clone().unwrap();
                                    let agent_dest_address = pending_mcmd.agent_dest_address.clone().unwrap();
                                    let service_name = pending_mcmd.service_name.clone().unwrap();

                                    info!("Proxying connection with ID {} from {} to {} ({}) for service '{}'.", connection_id, agent_from_address, server_from_address, agent_dest_address, service_name);

                                    //connection is proxied on separate thread
                                    tokio::spawn(async move {
                                        let mut a_s = agent_stream.lock().await;
                                        let mut s_s = server_stream.lock().await;
                                        let mut a_stream = &mut *a_s;
                                        let mut s_stream = &mut *s_s;
                                        match io::copy_bidirectional(&mut a_stream, &mut s_stream).await {
                                            Err(e) => {
                                                error!("An error occurred while proxying connection from {} to {} ({}) for service '{}' with connection ID {}: {}.", agent_from_address, server_from_address, agent_dest_address, service_name, connection_id, e);
                                            },
                                            Ok((bytes_from_agent, bytes_from_client)) => {
                                                info!("Proxy of connection for service '{}' from {} to {} ({}) with connection ID {} is closed: {} bytes from agent, {} bytes from client.", service_name, agent_from_address, server_from_address, agent_dest_address, connection_id, bytes_from_agent, bytes_from_client);
                                            }
                                        }
                                    });
                                } else {
                                    error!("Agent control thread attempted to connect with an ID that is not pending.");
                                }
                            },
                            CMD_CANCEL_CONNECTION => {
                                let connection_id = mcmd.connection_id.clone().unwrap();
                                if (pending_connections_by_id.contains_key(&connection_id)) {
                                    pending_connections_by_id.remove_entry(&connection_id);
                                }
                                info!("Cancelled connection ID {}.", connection_id);
                            },
                            CMD_DEREGISTER => {
                                let mut keys:Vec<String> = Vec::new();
                                for key in agent_control_threads_by_group_name.keys() {
                                    keys.push(String::from(key));
                                }
                                for key in keys {
                                    if (agent_control_threads_by_group_name[&key].contains_key(&agent_thread_id)) {
                                        let address = agent_control_threads_by_group_name[&key][&agent_thread_id].from_address.clone().unwrap();
                                        agent_control_threads_by_group_name.get_mut(&key).unwrap().remove_entry(&agent_thread_id);
                                        if (agent_control_threads_by_group_name[&key].len() > 0) {
                                            //don't remove
                                        } else {
                                            agent_control_threads_by_group_name.remove_entry(&key);
                                        }
                                        info!("Deregistered agent ({}).", address);
                                    }
                                }
                                reconcile_agents_and_services(server_service_configs.clone(), &agent_control_threads_by_group_name, &mut service_control_threads_by_service_name, sct_tx.clone());
                            },
                            _ => {
                                warn!("Agent ({}) command not handled: {}.", address, mcmd.name);
                            }
                        }
                    },
                    sct_option = sct_recv => {
                        let mcmd = sct_option.unwrap();
                        match mcmd.name.as_str() {
                            CMD_NEW_SERVICE => {
                                let service_name = mcmd.service_name.unwrap();
                                let sct = service_control_threads_by_service_name.get_mut(&service_name).unwrap();
                                sct.tx = mcmd.tx;
                                info!("Registered service: {}.", service_name);
                            },
                            CMD_DEREGISTER => {
                                let service_thread_id = mcmd.id.clone();
                                if (service_control_threads_by_service_name.contains_key(&service_thread_id)) {
                                    service_control_threads_by_service_name.remove_entry(&service_thread_id);
                                    info!("Deregistered service: {}.", service_thread_id);
                                }
                            },
                            CMD_CONNECT => {
                                let service_name = mcmd.id.clone();
                                let address = mcmd.from_address.unwrap();
                                if (service_control_threads_by_service_name.contains_key(&service_name)) {
                                    let sct = &service_control_threads_by_service_name[&service_name];
                                    let agent_dest_address = sct.agent_dest_address.clone().unwrap();
                                    let mut counter = sct.counter.unwrap();
                                    let connection_id = get_rand_guid();
                                    let pending_mcmd = AppCommand{ id: connection_id.clone(), service_name: Some(service_name.clone()), name: String::from(CMD_CONNECT), stream: mcmd.stream, agent_dest_address: Some(agent_dest_address.clone()), from_address: Some(address), ..AppCommand::default() };
                                    pending_connections_by_id.insert(connection_id.clone(), pending_mcmd);
                                    let mut supporting_agents:Vec<&AppCommand> = Vec::new();
                                    let agent_group_names = get_service_group_names(&service_name, &server_service_configs);

                                    //get array of all viable agent IDs
                                    for group_name in agent_group_names {
                                        if (agent_control_threads_by_group_name.contains_key(&group_name)) {
                                            for agent_id in agent_control_threads_by_group_name[&group_name].keys() {
                                                supporting_agents.push(&agent_control_threads_by_group_name[&group_name][agent_id]);
                                            }
                                        }
                                    }

                                    if (supporting_agents.len() > 0) {
                                        //choose the agent via round-robin modulus
                                        let chosen_agent = supporting_agents[counter.rem_euclid(supporting_agents.len())];
                                        counter = counter + 1;
                                        
                                        //update counter used to choose agent
                                        service_control_threads_by_service_name.get_mut(&service_name).unwrap().counter = Some(counter);
                                        
                                        let agent_address = chosen_agent.from_address.unwrap();
                                        let from_address = mcmd.from_address.unwrap();
                                        let group_name = chosen_agent.group_name.clone().unwrap();

                                        info!("Selected agent {} in group '{}' for connection from {} for connection ID {}. Notifying agent to proxy {}...", agent_address, group_name, from_address, connection_id, agent_dest_address);

                                        //tell agent that it needs to connect
                                        let agent_mcmd = AppCommand{ id: chosen_agent.id.clone(), name: String::from(CMD_CONNECT), connection_id: Some(connection_id.clone()), agent_dest_address: Some(agent_dest_address.clone()), ..AppCommand::default()  };
                                        match chosen_agent.tx.as_ref().unwrap().send(agent_mcmd).await {
                                            Err(e) => {
                                                error!("Could not send connect command to agent ({}) for service connection from ({}) with connection ID {}: {}.", agent_address, from_address, connection_id, e);
                                            },
                                            Ok(_result) => { }
                                        }
                                    } else {
                                        error!("There is no agent to handle the connection from {}.", address);
                                    }
                                }
                            },
                            _ => {
                                warn!("Service command not handled: {}.", mcmd.name);
                            }
                        }
                    }

                );
            }
        }
    }
    return Ok(());
}

fn load_server_services(cmd_line_server_configs:ServerConfigs) -> io::Result<ServerConfigs> {
    let mut server_service_configs = load_server_configs()?;

    //merge configs from the cmd line and the config file, if available
    for key in cmd_line_server_configs.keys() {
        let sconfig = cmd_line_server_configs[key].clone();
        add_server_config(&sconfig, &mut server_service_configs);
    }

    if (server_service_configs.len() > 0) {     //if no configs, we don't have any mappings, so return
        for key in server_service_configs.keys() {
            let server_config = &server_service_configs[key];
            info!("Service config - name: {}, agent group name: {}, server service bind address: {}, agent endpoint: {}.", server_config.service_name, server_config.agent_group_name, server_config.server_bind_endpoint, server_config.agent_service_endpoint);
        }
    } else {
        error!("No configurations specified on command line or in '{}'.", get_config_file_path().unwrap().display());
    }
    return Ok(server_service_configs);
}

//handles a connected agent and sends/receives messages to/from main sever thread
async fn agent_control_thread(id:String, tcp_stream:TcpStream, act_tx:mpsc::Sender<AppCommand>, address:SocketAddr, pre_shared_key:String) {
    let shared_stream = SharedTokioStream::new(Mutex::new(tcp_stream));
    let stream = &mut *shared_stream.lock().await;

    //return comms from main thread
    let (s_tx, mut act_rx) = mpsc::channel::<AppCommand>(1000);

    //keep alive timer
    let mut ka_timer = interval(Duration::from_millis(KEEP_ALIVE_INTERVAL_MS)); 

    //indicates agent authenticated with pre shared key
    let mut agent_authenticated = false;

    //indicates agent completed handshake - used for keep alive logic
    let mut agent_ready = false;

    //buffer used for reading lines - must be reset after successful calls
    let mut buf:Vec<u8> = Vec::new();

    //main event loop for handling the agent connection
    loop {
        let read_line = tokio_read_line(stream, &mut buf);
        let recv_cmd = act_rx.recv();
        let ka_timer_tick = ka_timer.tick();

        tokio::select!(
            result = read_line => {
                match result {
                    Err(e) => {
                        error!("Error reading from new agent ({}): {}.", address, e);
                        break;
                    },
                    Ok(()) => {
                        let bytes_read = buf.len();
                        if (bytes_read > 0) {
                            let l = String::from_utf8(buf).unwrap();
                            buf = Vec::new();
                            let line = rm_newline(l);
                            let command = line;
                            if (agent_authenticated) {
                                match command.as_str() {
                                    CMD_NEW_AGENT => {
                                        match tokio_read_line(stream, &mut buf).await {
                                            Err(e) => {
                                                error!("Error reading from new agent ({}): {}.", address, e);
                                                break;
                                            },
                                            Ok(()) => {
                                                let bytes_read = buf.len();
                                                if (bytes_read > 0) {
                                                    let l = String::from_utf8(buf).unwrap();
                                                    buf = Vec::new();
                                                    let line = rm_newline(l);
                                                    let group_name = line;
                                                    match tokio_write_line(stream, CMD_OK).await {
                                                        Err(e) => {
                                                            error!("Error writing CMD_OK to agent {}: {}.", address, e);
                                                            break;
                                                        },
                                                        Ok(_result) => { }
                                                    }
                                                    let mcmd = AppCommand{ id: id.clone(), name: String::from(CMD_NEW_AGENT), group_name: Some(group_name.clone()), tx: Some(s_tx.clone()), from_address: Some(address), ..AppCommand::default() };
                                                    match act_tx.send(mcmd).await {
                                                        Err(e) => {
                                                            error!("Could not communicate to parent thread ({}): {}.", address, e);
                                                            break;
                                                        },
                                                        Ok(_result) => { }
                                                    }
                                                    agent_ready = true;
                                                } else {
                                                    warn!("Agent connection closed ({}).", address);
                                                    break;
                                                }
                                            }
                                        }
                                    },
                                    CMD_CONNECT => {
                                        match tokio_read_line(stream, &mut buf).await {
                                            Err(e) => {
                                                error!("Error reading from agent connect ({}): {}.", address, e);
                                                break;
                                            },
                                            Ok(()) => {
                                                let bytes_read = buf.len();
                                                if (bytes_read > 0) {
                                                    let l = String::from_utf8(buf).unwrap();
                                                    let line = rm_newline(l);
                                                    let connection_id = line;
    
                                                    info!("Handling connection ID {} for agent connection from {}.", connection_id, address);
    
                                                    let mcmd = AppCommand{ id: id.clone(), name: String::from(CMD_CONNECT), connection_id: Some(connection_id.clone()), stream: Some(shared_stream.clone()), from_address: Some(address), ..AppCommand::default() };
                                                    match act_tx.send(mcmd).await {
                                                        Err(e) => {
                                                            error!("Could not communicate to parent thread during connect ({}): {}.", address, e);
                                                        },
                                                        Ok(_result) => { }
                                                    }
                                                    break;
                                                } else {
                                                    warn!("Agent ({}) connection closed during connect.", address);
                                                    break;
                                                }
                                            }
                                        }
                                    },
                                    CMD_CANCEL_CONNECTION => {
                                        match tokio_read_line(stream, &mut buf).await {
                                            Err(e) => {
                                                error!("Error reading from agent cancel connection ({}): {}.", address, e);
                                                break;
                                            },
                                            Ok(()) => {
                                                let bytes_read = buf.len();
                                                if (bytes_read > 0) {
                                                    let l = String::from_utf8(buf).unwrap();
                                                    let line = rm_newline(l);
                                                    let connection_id = line;
    
                                                    info!("Handling cancellation for connection ID {} for agent connection from {}.", connection_id, address);
    
                                                    let mcmd = AppCommand{ id: id.clone(), name: String::from(CMD_CANCEL_CONNECTION), connection_id: Some(connection_id.clone()), stream: Some(shared_stream.clone()), from_address: Some(address), ..AppCommand::default() };
                                                    match act_tx.send(mcmd).await {
                                                        Err(e) => {
                                                            error!("Could not communicate to parent thread during connection cancel for connection ID {} ({}): {}.", connection_id, address, e);
                                                        },
                                                        Ok(_result) => { }
                                                    }
                                                    break;
                                                } else {
                                                    warn!("Agent ({}) connection closed.", address);
                                                    break;
                                                }
                                            }
                                        }
                                    },
                                    _ => {
                                        error!("Unrecognized agent ({}) command: {}. Dropping connection.", address, command);
                                        break;
                                    }
                                }
                            } else { //not authenticated
                                match command.as_str() {
                                    CMD_PSK => {
                                        match tokio_read_line(stream, &mut buf).await {
                                            Err(e) => {
                                                error!("Error reading from agent cancel connection ({}): {}.", address, e);
                                                break;
                                            },
                                            Ok(()) => {
                                                let bytes_read = buf.len();
                                                if (bytes_read > 0) {
                                                    let l = String::from_utf8(buf).unwrap();
                                                    buf = Vec::new();
                                                    let line = rm_newline(l);
                                                    let psk = line;

                                                    if (psk.eq(&pre_shared_key)) {
                                                        agent_authenticated = true;
                                                    } else {
                                                        warn!("Agent ({}) sent wrong pre shared key. Closing connection.", address);
                                                        break;
                                                    }
                                                } else {
                                                    warn!("Agent ({}) connection closed.", address);
                                                    break;
                                                }
                                            }
                                        }
                                    },
                                    _ => {
                                        error!("Agent ({}) tried command {} without first authenticating. Dropping connection.", address, command);
                                        break;
                                    }
                                }
                            }
                        } else {
                            warn!("Agent connection closed ({}).", address);
                            break;
                        }
                    }
                }
            },
            _result = ka_timer_tick => {
                if (agent_ready) {
                    match tokio_write_line(stream, String::from(CMD_KEEP_ALIVE).as_str()).await {
                        Err(e) => {
                            error!("Could not write to agent ({}): {}.", address, e);
                            break;
                        },
                        Ok(_result) => { }
                    }
                }
            },
            recv_cmd_option = recv_cmd => {
                let mcmd = recv_cmd_option.unwrap();
                let cmd = mcmd.name.clone();
                match cmd.as_str() {
                    CMD_CONNECT => {
                        let connect_cmd = String::from(CMD_CONNECT) + "\n" + &mcmd.agent_dest_address.unwrap() + "\n" + &mcmd.connection_id.unwrap();
                        match tokio_write_line(stream, connect_cmd.as_str()).await {
                            Err(e) => {
                                error!("Could not write {} to agent ({}): {}.", CMD_CONNECT, address, e);
                                break;
                            },
                            Ok(_result) => { 
                            }
                        }
                    },
                    _ => {
                        error!("Agent control thread ({}) received command that isn't yet implemented: {}.", address, cmd);
                        break;
                    }
                }
            }
        );
    }
}

async fn service_control_thread(id:String, bind_address:String, sct_tx:mpsc::Sender<AppCommand>) {
    let endpoint = bind_address;

    //return comms from main thread
    let (s_tx, mut sct_rx) = mpsc::channel::<AppCommand>(1000);

    //listen on service port for new connections
    match TcpListener::bind(&endpoint).await {
        Err(e) => {
            error!("Could not listen on {} for service {}. Error: {}", endpoint, id, e);
        },
        Ok(listener) => {
            info!("Listening on {} for service {}.", endpoint, id);

            //register
            let mcmd = AppCommand{ id: id.clone(), name: String::from(CMD_NEW_SERVICE), service_name: Some(id.clone()), tx: Some(s_tx.clone()), ..AppCommand::default() };
            match sct_tx.send(mcmd).await {
                Err(e) => {
                    error!("Could not communicate to parent thread: {}.", e);
                    return;
                },
                Ok(_result) => { }
            }

            //event loop
            loop {
                let listener_accept = listener.accept();
                let sct_recv = sct_rx.recv();

                tokio::select!(
                    result = listener_accept => {
                        match result {
                            Err(e) => {
                                error!("Error accepting connection for service {}: {}.", id, e);
                                break;
                            },
                            Ok((stream, address)) => {
                                info!("Accepted connection for service {} from: {}.", id, address);
                                let mcmd = AppCommand{ id: id.clone(), name: String::from(CMD_CONNECT), stream: Some(SharedTokioStream::new(Mutex::new(stream))), from_address: Some(address), service_name: Some(id.clone()), ..AppCommand::default() };
                                match sct_tx.send(mcmd).await {
                                    Err(e) => {
                                        error!("Could not send connect command to main thread for new service connection ({}): {}.", address, e);
                                        break;
                                    },
                                    Ok(_result) => { }
                                }
                            }
                        }
                    },
                    sct_option = sct_recv => {
                        let mcmd = sct_option.unwrap();
                        let cmd = mcmd.name.as_str();
                        match cmd {
                            CMD_SHUTDOWN => {
                                info!("Shutting down control thread for service {}...", id);
                                break;
                            },
                            _ => {
                                error!("Service command not recognized: {}.", mcmd.name);
                                break;
                            }
                        }
                    }
                );
            }
        }
    }
}

fn reconcile_agents_and_services(service_configs:ServerConfigs, agent_control_threads_by_group_name:&AgentControlThreadsByGroupName, service_control_threads_by_service_name:&mut ServiceControlThreadsByServiceName, sct_tx:mpsc::Sender<AppCommand>) {
    //for every service, check to see if there is a supporting agent, and if so, spawn a thread, and if not, stop the thread if there is one running
    for key in service_configs.keys() {
        let sc = &service_configs[key];
        let service_name = sc.service_name.clone();
        let group_name = sc.agent_group_name.clone();
        let s_group_name = group_name.as_str();
        let agent_dest_address = sc.agent_service_endpoint.clone();

        //check to see if there is a registered agent that can support this service
        let mut can_support = false;
        for agent_gn in agent_control_threads_by_group_name.keys() {
            if (agent_control_threads_by_group_name[agent_gn].len() > 0) {
                if ((agent_gn == "") || (agent_gn == s_group_name)) {
                    can_support = true;
                }
            }
        }

        if (can_support) {
            if (service_control_threads_by_service_name.contains_key(&service_name)) {
                //already running a thread for this service
            } else {
                info!("Registering new service: {}...", service_name);
                
                //add the entry
                let mcmd = AppCommand{ id: service_name.clone(), agent_dest_address: Some(agent_dest_address.clone()), name: String::from(""), counter: Some(1), ..AppCommand::default() };
                service_control_threads_by_service_name.insert(service_name.clone(), mcmd);

                let new_sct_tx = sct_tx.clone();
                let sname = service_name.clone();
                let sbe = sc.server_bind_endpoint.clone();
                tokio::spawn(async move { 
                    service_control_thread(sname.clone(), sbe, new_sct_tx.clone()).await;

                    //deregister
                    let mcmd = AppCommand{ id: sname.clone(), name: String::from(CMD_DEREGISTER), ..AppCommand::default() };
                    match new_sct_tx.send(mcmd).await {
                        Err(e) => {
                            error!("Could not send deregister command for service {}: {}.", sname, e);
                        },
                        Ok(_result) => { }
                    }
                });
            }
        } else {
            if (service_control_threads_by_service_name.contains_key(&service_name)) {
                //shut down the service thread
                let sct_tx = service_control_threads_by_service_name[&service_name].tx.clone().unwrap();
                let mcmd = AppCommand{ id: service_name.clone(), name: String::from(CMD_SHUTDOWN), ..AppCommand::default() };
                tokio::spawn( async move {
                    let id = mcmd.id.clone();
                    match sct_tx.send(mcmd).await {
                        Err(e) => {
                            error!("Could not send shutdown command to service {}: {}.", id, e);
                        },
                        Ok(_result) => { }
                    }
                });
            }
        }
    }
}

fn get_service_group_names(service_name:&String, services:&ServerConfigs) -> Vec<String> {
    let mut group_names:Vec<String> = Vec::new();
    for key in services.keys() {
        let s = &services[key];
        if (s.service_name.eq(service_name)) {
            group_names.push(s.service_name.clone());
        }
    }
    group_names.push(String::from(""));
    return group_names;
}

fn get_connected_agent_count(agent_control_threads_by_group_name:&AgentControlThreadsByGroupName) -> usize {
    let mut count:usize = 0;
    for agent_gn in agent_control_threads_by_group_name.keys() {
        count += agent_control_threads_by_group_name[agent_gn].len();
    }
    return count;
}