use tokio::{io, io::{BufReader, AsyncBufReadExt, AsyncWriteExt}, net::{TcpStream}, time::{interval, sleep, Duration}};
use log::{error, info, warn};

use crate::constants::*;
use crate::utils::*;

pub async fn run_agent(group_name:String, endpoint:String, pre_shared_key:String) -> io::Result<()> {
    info!("Running in agent mode. Group name: {}, Server Endpoint: {}.", group_name, &endpoint);

    loop {
        info!("Connecting to {}...", &endpoint);

        match TcpStream::connect(&endpoint).await {
            Err(e) => {
                error!("Failed to connect to server: {}", e);
            },
            Ok(stream) => {
                let (read_stream, write_stream) = stream.into_split();

                let mut reader = BufReader::new(read_stream);
                let mut writer = write_stream;
                
                let mut buf = String::new();
                let mut handshake_complete = false;

                //set up connection with server
                match writer.write_all((CMD_PSK.to_owned() + "\n" + &pre_shared_key + "\n" + &CMD_NEW_AGENT.to_owned() + "\n" + group_name.as_str() + "\n").as_bytes()).await {
                    Err(e) => {
                        error!("Could not send CMD_NEW_AGENT to server: {}.", e);
                    },
                    Ok(_result) => {
                        match reader.read_line(&mut buf).await {
                            Err(e) => {
                                error!("Error reading from control connection: {}", e);
                            },
                            Ok(bytes_read) => {
                                if (bytes_read > 0) {
                                    let response = rm_newline(buf);
                                    match response.as_str() {
                                        CMD_OK => {
                                            info!("Connected to {} and awaiting connection commands.", endpoint);
                                            handshake_complete = true;
                                        },
                                        _ => {
                                            error!("Unrecognized server response: {}. Aborting.", response);
                                        }
                                    }
                                } else {
                                    error!("Agent connection closed.");
                                }
                            }
                        }

                        //keep alive timer
                        let mut ka_timer = interval(Duration::from_millis(KEEP_ALIVE_INTERVAL_MS)); 
        
                        //handle commands from server, send keepalive
                        while (handshake_complete) {
                            buf = String::new();

                            let ka_timer_tick = ka_timer.tick();
                            let read_line = reader.read_line(&mut buf);
                    
                            tokio::select!(
                                result = read_line => {
                                    match (result) {
                                        Err(e) => {
                                            error!("Error reading from control connection: {}", e);
                                            break;
                                        },
                                        Ok(bytes_read) => {
                                            if (bytes_read > 0) {
                                                let response = rm_newline(buf);
                                                match response.as_str() {
                                                    CMD_OK => {
                                                        //do nothing
                                                    },
                                                    CMD_KEEP_ALIVE => {
                                                        //do nothing
                                                    },
                                                    CMD_CONNECT => {
                                                        let mut dest_address = String::new();
                                                        let mut connection_id = String::new();
                                                        match reader.read_line(&mut dest_address).await {
                                                            Err(e) => {
                                                                error!("Could not read dest address from connect command: {}.", e);
                                                            },
                                                            Ok(bytes_read) => {
                                                                if (bytes_read > 0) {
                                                                    dest_address = rm_newline(dest_address);
                                                                    match reader.read_line(&mut connection_id).await {
                                                                        Err(e) => {
                                                                            error!("Could not read connection ID from connect command: {}.", e);
                                                                        },
                                                                        Ok(bytes_read) => {
                                                                            if (bytes_read > 0) {
                                                                                connection_id = rm_newline(connection_id);
                                                                                let server_endpoint = endpoint.clone();
                                                                                let psk = pre_shared_key.clone();
                                                                                tokio::spawn(async move {
                                                                                    match proxy_connection(connection_id, server_endpoint, dest_address, psk).await {
                                                                                        Err(e) => {
                                                                                            error!("Problem during proxy connection: {}.", e);
                                                                                        },
                                                                                        Ok(_result) => { }
                                                                                    }
                                                                                });
                                                                            } else {
                                                                                warn!("Connection to server was closed.");
                                                                            }
                                                                        }
                                                                    }
                                                                } else {
                                                                    warn!("Connection to server closed.");
                                                                }
                                                            }
                                                        }
                                                    },
                                                    _ => {
                                                        //process_command(&response);
                                                    }
                                                }
                                            } else {
                                                warn!("Agent control connection closed.");
                                                break;
                                            }
                                        }
                                    }
                                },
                                _result = ka_timer_tick => {
                                    match writer.write_all(CMD_KEEP_ALIVE.as_bytes()).await {
                                        Err(e) => {
                                            error!("Could not write keep alive to server ({}): {}.", endpoint, e);
                                            break;
                                        },
                                        Ok(_result) => { }
                                    }
                                }
                            );
                        }
                    }
                };
            }
        }

        info!("Waiting {}ms before attempting reconnect...", WAIT_RECONNECT_MS);
        sleep(Duration::from_millis(WAIT_RECONNECT_MS)).await;
    }
}

async fn proxy_connection(connection_id:String, server_endpoint:String, dest_address:String, pre_shared_key:String) -> io::Result<()> {
    match TcpStream::connect(&dest_address).await {
        Err(e) => {
            error!("Error connecting to dest endpoint {} for connection ID {}: {}.", dest_address, connection_id, e);
            info!("Sending cancellation to {} for connection ID {}...", server_endpoint, connection_id);
            match send_cancel_connection(&server_endpoint, &connection_id).await {
                Err(e) => {
                    error!("Could not send cancellation to {} for connection ID {}: {}.", server_endpoint, connection_id, e);
                },
                Ok(_result) => { 
                    info!("Successfully sent cancellation to {} for connection ID {}.", server_endpoint, connection_id);
                }
            }
        },
        Ok(mut agent_stream) => {
            match TcpStream::connect(&server_endpoint).await {
                Err(e) => {
                    error!("Could not connect to server {} to begin proxy for connection ID {}: {}.", server_endpoint, connection_id, e);
                },
                Ok(mut server_stream) => {
                    info!("Proxying connection between {} and {} with connection ID {}.", server_endpoint, dest_address, connection_id);
        
                    let connection_cmd = String::from(CMD_PSK) + "\n" + &pre_shared_key + "\n" + &String::from(CMD_CONNECT) + "\n" + &connection_id + "\n";
                    server_stream.write_all(connection_cmd.as_bytes()).await?;
        
                    tokio::spawn(async move {
                        match io::copy_bidirectional(&mut agent_stream, &mut server_stream).await {
                            Err(e) => {
                                error!("An error occurred during stream proxy of {} to {} with connection ID {}: {}.", dest_address, server_endpoint, connection_id, e);
                            },
                            Ok((agent_to_server_bytes, server_to_agent_bytes)) => {
                                info!("Proxy connection of {} to {} with connection ID {} is closed: {} bytes sent, {} bytes received.", server_endpoint, dest_address, connection_id, agent_to_server_bytes, server_to_agent_bytes);
                            }
                        }
                    });
                }
            }
        }
    }
    return Ok(());
}

async fn send_cancel_connection(server_endpoint:&String, connection_id:&String) -> io::Result<()> {
    let mut server_stream = TcpStream::connect(server_endpoint).await?;
    let cancel_cmd = String::from(CMD_CANCEL_CONNECTION) + "\n" + &connection_id + "\n";
    server_stream.write_all(cancel_cmd.as_bytes()).await?;
    return Ok(());
}