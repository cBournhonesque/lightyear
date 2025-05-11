use bevy::prelude::*;
use bytes::BytesMut;
use lightyear_connection::client::Disconnected;
use lightyear_connection::client_of::{ClientOf, Server};
use lightyear_link::{Link, LinkSet, LinkStart, Linked, Linking, Unlink, Unlinked};

use alloc::sync::Arc;
use async_compat::Compat;
use bevy::platform::collections::HashMap;
use bevy::tasks::{IoTaskPool, Task};
use core::net::SocketAddr;
use lightyear_link::prelude::ServerLink;
use std::io;
use std::sync::Mutex;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info};
use wtransport::datagram::Datagram;
use wtransport::endpoint::endpoint_side;
use wtransport::{self, Endpoint, Identity, ServerConfig};

/// Maximum transmission units; maximum size in bytes of a WebTransport datagram
const MTU: usize = 1200; // WebTransport usually has slightly smaller MTU than UDP

#[derive(Component)]
pub struct ServerWebTransportIo {
    local_addr: SocketAddr,
    certificate: Identity,
    inner: Option<ServerWebTransportInner>,
}

struct ServerWebTransportInner {
    task: Task<()>,
    status_rx: UnboundedReceiver<ServerIoEvent>,
    close_tx: UnboundedSender<ServerIoEvent>,
    client_senders: Arc<Mutex<HashMap<SocketAddr, UnboundedSender<BytesMut>>>>,
    from_clients_rx: UnboundedReceiver<(BytesMut, SocketAddr)>,
    from_clients_tx: UnboundedSender<(BytesMut, SocketAddr)>,
}


impl ServerWebTransportIo {
    pub fn new(local_addr: SocketAddr, certificate: Identity) -> Self {
        ServerWebTransportIo {
            local_addr,
            certificate,
            inner: None,
        }
    }
}

pub struct ServerWebTransportPlugin;

impl Plugin for ServerWebTransportPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, Self::receive.in_set(LinkSet::Receive));
        app.add_systems(PreUpdate, Self::send.in_set(LinkSet::Send));
        app.add_systems(PreUpdate, Self::handle_connections);
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("transport is not connected. Did you call connect()?")]
    NotConnected,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
    #[error(transparent)]
    WebTransport(#[from] wtransport::error::ConnectingError),
    #[error("could not send message via channel: {0}")]
    Channel(String),
    #[error("requested by user")]
    UserRequest,
}

enum ServerIoEvent {
    ServerConnected,
    ServerDisconnected(Error),
    ClientDisconnected(SocketAddr),
}

impl ServerWebTransportPlugin {
    fn link_start(
        trigger: Trigger<LinkStart>,
        mut query: Query<&mut ServerWebTransportIo, With<Unlinked>>,
        mut commands: Commands,
    ) -> Result {
        if let Ok(mut io) = query.get_mut(trigger.target()) {
            // channels used to cancel the task
            let (close_tx, close_rx) = async_channel::unbounded();
            // channels used to check the status of the io task
            let (status_tx, status_rx) = async_channel::unbounded();
            let to_client_senders = Arc::new(Mutex::new(HashMap::default()));
            let addr_to_task = Arc::new(Mutex::new(HashMap::default()));
            let (to_client_sender, to_client_receiver) =
            mpsc::unbounded_channel::<(Box<[u8]>, SocketAddr)>();
            let (from_client_sender, from_client_receiver) = mpsc::unbounded_channel();
            
            // Setup server config
            let mut config = ServerConfig::builder()
                .with_bind_address(io.local_addr)
                .with_identity(io.certificate.clone_identity())
                .build();

            // Set appropriate MTU
            let mut quic_config = wtransport::quinn::TransportConfig::default();
            quic_config.initial_mtu(MTU as u16).min_mtu(MTU as u16);
            config
                .quic_config_mut()
                .transport_config(Arc::new(quic_config));
            
            info!("WebTransport server started on {}", io.local_addr);
            // need to run this with Compat because it requires the tokio reactor
            let task = IoTaskPool::get()
                .spawn(Compat::new(async move {
                let endpoint = match wtransport::Endpoint::server(config) {
                    Ok(e) => e,
                    Err(e) => {
                        status_tx
                            .send(ServerIoEvent::ServerDisconnected(e.into()))
                            .await
                            .unwrap();
                        return;
                    }
                };
                info!("Starting server webtransport task");
                status_tx.send(ServerIoEvent::ServerConnected).await.unwrap();
                loop {
                    tokio::select! {
                        // event from netcode
                        Ok(event) = close_rx.recv() => {
                            match event {
                                ServerIoEvent::ServerDisconnected(e) => {
                                    debug!("Stopping all webtransport io tasks. Reason: {:?}", e);
                                    // drop all tasks so that they can be cleaned up
                                    drop(addr_to_task);
                                    return;
                                }
                                ServerIoEvent::ClientDisconnected(addr) => {
                                    debug!("Stopping webtransport io task associated with address: {:?} because we received a disconnection signal from netcode", addr);
                                    addr_to_task.lock().unwrap().remove(&addr);
                                }
                                _ => {}
                            }
                        }
                        // new client connecting
                        incoming_session = endpoint.accept() => {
                            // TODO: let user choose if they want to accept the connection or not
                            let Ok(session_request) = incoming_session
                                .await
                                .inspect_err(|e| {
                                    error!("failed to accept new client: {:?}", e);
                                }) else {
                                continue;
                            };
                            let Ok(connection) = session_request
                                .accept()
                                .await
                                .inspect_err(|e| {
                                    error!("failed to accept new client: {:?}", e);
                                }) else {
                                continue;
                            };
                            let client_addr = connection.remote_address();
                            let connection = Arc::new(connection);
                            let from_client_sender = from_client_sender.clone();
                            let to_client_senders = to_client_senders.clone();
                            let task = IoTaskPool::get()
                                .spawn(Compat::new(WebTransportServerSocket::handle_client(
                                    connection,
                                    from_client_sender,
                                    to_client_senders,
                                    status_tx.clone(),
                                )));
                            addr_to_task.lock().unwrap().insert(client_addr, task);
                        }
                    }
                }
                ()
            }));
            
            let inner = Some(ServerWebTransportInner {
                task,
                status_rx,
                close_tx,
                client_senders: to_client_senders,
                from_clients_rx: from_clients_rx.clone(),
                from_clients_tx: from_clients_tx.clone(),
            });
            
            
            commands.entity(trigger.target()).insert(Linked);
        }
        Ok(())
    }

    fn poll_server(
        mut query: Query<(&mut ServerWebTransportIo, Has<Linking>, Has<Linked>)>,
    ) {


    }

    fn linking(
        mut query: Query<&mut ServerWebTransportIo, With<Linking>>,
        mut commands: Commands,
    ) {
        for mut task in &mut transform_tasks {
            if let Some(mut commands_queue) = block_on(future::poll_once(&mut task.0)) {
                // append the returned command queue to have it execute later
                commands.append(&mut commands_queue);
            }
        }
    }

    fn unlink(
        trigger: Trigger<Unlink>,
        mut query: Query<(Entity, &mut ServerWebTransportIo), Without<Unlinked>>,
        mut commands: Commands,
    ) {
        if let Ok((server, mut io)) = query.get_mut(trigger.target()) {
            info!("Server WebTransport link closed");
            // TODO: drop all tasks
            io.endpoint = None;
            io.from_clients_rx = None;
            io.from_clients_tx = None;
            commands.entity(server).despawn_related::<ServerLink>();
            commands.entity(trigger.target()).insert(Unlinked {
                reason: Some("User request".to_string()),
            });
        }
    }

    fn send(
        mut server_query: Query<(&mut ServerWebTransportIo, &Server), With<Linked>>,
        mut link_query: Query<&mut Link>,
    ) {
        server_query
            .par_iter_mut()
            .for_each(|(mut server_io, server)| {
                server.collection().iter().for_each(|client_entity| {
                    let Ok(mut link) = link_query.get_mut(*client_entity) else {
                        error!("Client entity {} not found in link query", client_entity);
                        return;
                    };

                    let Some(remote_addr) = link.remote_addr else {
                        error!("Client entity {} has no remote address", client_entity);
                        return;
                    };

                    let client_senders = server_io.client_senders.lock().unwrap();
                    if let Some(sender) = client_senders.get(&remote_addr) {
                        link.send.drain(..).for_each(|payload| {
                            // Create a copy of the payload data for sending
                            let mut bytes = BytesMut::new();
                            bytes.extend_from_slice(payload.as_ref());
                            if let Err(e) = sender.send(bytes) {
                                error!(
                                    "Error sending WebTransport datagram to {}: {}",
                                    remote_addr, e
                                );
                            }
                        });
                    }
                });
            });
    }

    fn receive(
        time: Res<Time<Real>>,
        mut commands: Commands,
        mut server_query: Query<(Entity, &mut ServerWebTransportIo, &Server)>,
        mut link_query: Query<&mut Link>,
    ) {
        server_query
            .par_iter_mut()
            .for_each(|(server_entity, mut server_io, server)| {
                let Some(mut from_clients_rx) = server_io.from_clients_rx.take() else {
                    return;
                };

                // Process all pending messages
                while let Ok((payload, address)) = from_clients_rx.try_recv() {
                    // Check if we already have this client
                    let entity = match server.get_client(peer_id) {
                        Some(entity) => {
                            // Existing client, add message to their link
                            if let Ok(mut link) = link_query.get_mut(entity) {
                                link.recv.push(payload.freeze(), time.elapsed());
                            } else {
                                error!(
                                    "Received WebTransport packet for unknown entity: {}",
                                    entity
                                );
                            }
                            continue;
                        }
                        None => {
                            // New client, create a new link
                            debug!("Received WebTransport packet from new address: {}", address);
                            let mut link = Link::new(address, None);
                            link.recv.push(payload.freeze(), time.elapsed());

                            // Spawn a new entity for this client
                            commands
                                .spawn((
                                    ClientOf {
                                        server: server_entity,
                                    },
                                    link,
                                ))
                                .id()
                        }
                    };
                }

                // Put the receiver back
                server_io.from_clients_rx = Some(from_clients_rx);
            });
    }

    fn handle_connections(mut server_query: Query<&mut ServerWebTransportIo>) {
        for mut server_io in server_query.iter_mut() {
            // Only process if we have an endpoint
            let Some(endpoint) = &server_io.endpoint else {
                continue;
            };

            // Create a task to accept new connections if not already running
            let endpoint = endpoint.clone();
            let client_senders = server_io.client_senders.clone();
            let from_clients_tx = match &server_io.from_clients_tx {
                Some(tx) => tx.clone(),
                None => continue,
            };

            tokio::spawn(async move {
                loop {
                    // Accept new connection
                    let incoming_session = match endpoint.accept().await {
                        Ok(session) => session,
                        Err(e) => {
                            error!("Error accepting WebTransport connection: {}", e);
                            continue;
                        }
                    };

                    // Accept the session request
                    let connection = match incoming_session.await.accept().await {
                        Ok(conn) => conn,
                        Err(e) => {
                            error!("Error establishing WebTransport connection: {}", e);
                            continue;
                        }
                    };

                    let client_addr = connection.remote_address();
                    info!("New WebTransport connection from {}", client_addr);

                    // Create channels for this client
                    let (client_tx, mut client_rx) = mpsc::unbounded_channel::<BytesMut>();
                    client_senders
                        .lock()
                        .unwrap()
                        .insert(client_addr, client_tx);

                    // Clone what we need for the client task
                    let connection = Arc::new(connection);
                    let conn_recv = connection.clone();
                    let conn_send = connection.clone();
                    let from_clients_tx = from_clients_tx.clone();

                    // Spawn task to receive messages from this client
                    tokio::spawn(async move {
                        while let Ok(datagram) = conn_recv.receive_datagram().await {
                            let mut bytes = BytesMut::new();
                            bytes.extend_from_slice(datagram.payload());
                            if let Err(e) = from_clients_tx.send((bytes, client_addr)) {
                                error!("Error forwarding client message: {}", e);
                                break;
                            }
                        }
                        info!("Client receive task ended for {}", client_addr);
                    });

                    // Spawn task to send messages to this client
                    tokio::spawn(async move {
                        while let Some(payload) = client_rx.recv().await {
                            if let Err(e) = conn_send.send_datagram(&payload) {
                                error!("Error sending to client {}: {}", client_addr, e);
                                break;
                            }
                        }
                        info!("Client send task ended for {}", client_addr);
                    });
                }
            });
        }
    }
}
