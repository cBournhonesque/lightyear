use bevy::prelude::*;
use bytes::BytesMut;
use lightyear_connection::client::Disconnected;
use lightyear_connection::client_of::{ClientOf, Server};
use lightyear_link::{Link, LinkSet};

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
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
    endpoint: Option<Arc<Endpoint<endpoint_side::Server>>>,
    // Map of client addresses to their message senders
    client_senders: Arc<Mutex<HashMap<SocketAddr, UnboundedSender<BytesMut>>>>,
    // Receiver for incoming messages from all clients
    from_clients_rx: Option<UnboundedReceiver<(BytesMut, SocketAddr)>>,
    // Sender for outgoing messages to be distributed to clients
    from_clients_tx: Option<UnboundedSender<(BytesMut, SocketAddr)>>,
}

impl ServerWebTransportIo {
    pub fn new(local_addr: SocketAddr, certificate: Identity) -> Self {
        let (from_clients_tx, from_clients_rx) = mpsc::unbounded_channel();
        
        ServerWebTransportIo {
            local_addr,
            certificate,
            endpoint: None,
            client_senders: Arc::new(Mutex::new(HashMap::new())),
            from_clients_rx: Some(from_clients_rx),
            from_clients_tx: Some(from_clients_tx),
        }
    }
    
    pub fn start(&mut self) -> Result<(), wtransport::error::Error> {
        // Setup server config
        let mut config = ServerConfig::builder()
            .with_bind_address(self.local_addr)
            .with_identity(self.certificate.clone())
            .build();
        
        // Set appropriate MTU
        let mut quic_config = wtransport::quinn::TransportConfig::default();
        quic_config
            .initial_mtu(MTU as u16)
            .min_mtu(MTU as u16);
        config.quic_config_mut().transport_config(Arc::new(quic_config));
        
        // Create the endpoint
        let endpoint = Endpoint::server(config)?;
        info!("WebTransport server started on {}", self.local_addr);
        self.endpoint = Some(Arc::new(endpoint));
        Ok(())
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

impl ServerWebTransportPlugin {

    fn send(
        mut server_query: Query<(&mut ServerWebTransportIo, &Server)>,
        mut link_query: Query<&mut Link, Without<Disconnected>>
    ) {
        server_query.par_iter_mut().for_each(|(mut server_io, server)| {
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
                            error!("Error sending WebTransport datagram to {}: {}", remote_addr, e);
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
        server_query.par_iter_mut().for_each(|(server_entity, mut server_io, server)| {
            let Some(mut from_clients_rx) = server_io.from_clients_rx.take() else {
                return;
            };
            
            // Process all pending messages
            while let Ok((payload, address)) = from_clients_rx.try_recv() {
                let peer_id = PeerId::IP(address);
                
                // Check if we already have this client
                let entity = match server.get_client(peer_id) {
                    Some(entity) => {
                        // Existing client, add message to their link
                        if let Ok(mut link) = link_query.get_mut(entity) {
                            link.recv.push(payload.freeze(), time.elapsed());
                        } else {
                            error!("Received WebTransport packet for unknown entity: {}", entity);
                        }
                        continue;
                    },
                    None => {
                        // New client, create a new link
                        debug!("Received WebTransport packet from new address: {}", address);
                        let mut link = Link::new(address, None);
                        link.recv.push(payload.freeze(), time.elapsed());
                        
                        // Spawn a new entity for this client
                        commands.spawn((
                            ClientOf {
                                server: server_entity,
                                id: peer_id,
                            },
                            link,
                        )).id()
                    }
                };
            }
            
            // Put the receiver back
            server_io.from_clients_rx = Some(from_clients_rx);
        });
    }

    fn handle_connections(
        mut server_query: Query<&mut ServerWebTransportIo>,
    ) {
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
                    client_senders.lock().unwrap().insert(client_addr, client_tx);
                    
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
