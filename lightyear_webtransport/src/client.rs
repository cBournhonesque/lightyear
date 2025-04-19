/*! WebTransport client implementation */
#![cfg_attr(not(feature = "std"), no_std)]

use async_compat::Compat;
use bevy::ecs::query::QueryEntityError;
use bevy::prelude::*;
use bevy::tasks::IoTaskPool;
use bytes::{Bytes, BytesMut};
use core::net::SocketAddr;
use lightyear_connection::client::Disconnected;
use lightyear_connection::id::PeerId;
use lightyear_link::{Link, LinkSet};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tracing::{debug, error, info, trace};
use wtransport;
use wtransport::datagram::Datagram;
use wtransport::error::ConnectingError;
use wtransport::ClientConfig;

/// Maximum transmission units for WebTransport (must be at least 1200 bytes for QUIC)
pub(crate) const MTU: usize = 1200;

#[derive(Component)]
pub struct ClientWebTransportIo {
    local_addr: SocketAddr,
    server_addr: SocketAddr,
    to_server_sender: mpsc::UnboundedSender<Box<[u8]>>,
    from_server_receiver: mpsc::UnboundedReceiver<Datagram>,
    buffer: BytesMut,
    close_sender: async_channel::Sender<()>,
}

impl ClientWebTransportIo {
    pub fn new(local_addr: SocketAddr, server_addr: SocketAddr) -> Result<Self, std::io::Error> {
        let (to_server_sender, to_server_receiver) = mpsc::unbounded_channel::<Box<[u8]>>();
        let (from_server_sender, from_server_receiver) = mpsc::unbounded_channel();
        // channels used to cancel the task
        let (close_tx, close_rx) = async_channel::bounded(1);
        
        IoTaskPool::get().spawn(Compat::new(async move {
            let mut config = ClientConfig::builder()
                .with_bind_address(local_addr)
                .with_no_cert_validation()
                .build();
            let mut quic_config = wtransport::quinn::TransportConfig::default();
            quic_config
                .initial_mtu(MTU as u16)
                .min_mtu(MTU as u16);
            config.quic_config_mut().transport_config(Arc::new(quic_config));
            
            let server_url = format!("https://{}", server_addr);
            info!("Connecting to server via webtransport at server url: {}", &server_url);
            
            let endpoint = match wtransport::Endpoint::client(config) {
                Ok(e) => e,
                Err(e) => {
                    error!("Error creating webtransport endpoint: {:?}", e);
                    return;
                }
            };

            tokio::select! {
                _ = close_rx.recv() => {
                    info!("WebTransport connection closed. Reason: client requested disconnection.");
                    return;
                }
                connection = endpoint.connect(&server_url) => {
                    let connection = match connection {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Error creating webtransport connection: {:?}", e);
                            return;
                        }
                    };
                    
                    info!("Connected to WebTransport server at {}", server_addr);
                    let connection = Arc::new(connection);

                    // Spawn a task for receiving datagrams
                    let connection_recv = connection.clone();
                    let recv_handle = IoTaskPool::get().spawn(Compat::new(async move {
                        loop {
                            match connection_recv.receive_datagram().await {
                                Ok(data) => {
                                    trace!("Received datagram from server: {} bytes", data.len());
                                    from_server_sender.send(data).unwrap_or_else(|e| {
                                        error!("Failed to forward received datagram: {:?}", e);
                                    });
                                }
                                Err(e) => {
                                    error!("Receive datagram error: {:?}", e);
                                    return;
                                }
                            }
                        }
                    }));
                    
                    // Spawn a task for sending datagrams
                    let connection_send = connection.clone();
                    let send_handle = IoTaskPool::get().spawn(Compat::new(async move {
                        while let Some(msg) = to_server_receiver.recv().await {
                            trace!("Sending datagram to server: {} bytes", msg.len());
                            connection_send.send_datagram(msg).unwrap_or_else(|e| {
                                error!("Failed to send datagram: {:?}", e);
                            });
                        }
                    }));
                    
                    // Wait for a close signal or connection closure
                    tokio::select! {
                        reason = connection.closed() => {
                            info!("WebTransport connection closed. Reason: {reason:?}");
                        },
                        _ = close_rx.recv() => {
                            info!("WebTransport connection closed. Reason: client requested disconnection");
                        }
                    }
                    
                    // Clean up tasks
                    recv_handle.cancel().await;
                    send_handle.cancel().await;
                    debug!("WebTransport tasks shut down");
                }
            }
        })).detach();

        Ok(ClientWebTransportIo {
            local_addr,
            server_addr,
            to_server_sender,
            from_server_receiver,
            buffer: BytesMut::with_capacity(MTU),
            close_sender: close_tx,
        })
    }
}

pub struct ClientWebTransportPlugin;

impl ClientWebTransportPlugin {
    fn send(
        mut client_query: Query<(&mut ClientWebTransportIo, &mut Link), Without<Disconnected>>
    ) {
        client_query.par_iter_mut().for_each(|(client_io, mut link)| {
            link.send.drain(..).for_each(|send_payload| {
                let data = send_payload.to_vec().into_boxed_slice();
                client_io.to_server_sender.send(data).unwrap_or_else(|e| {
                    error!("Error sending WebTransport packet: {}", e);
                });
            });
        });
    }

    fn receive(
        time: Res<Time<Real>>,
        mut client_query: Query<(&mut ClientWebTransportIo, &mut Link)>
    ) {
        client_query.par_iter_mut().for_each(|(mut client_io, mut link)| {
            // Process all available messages from the WebTransport connection
            loop {
                match client_io.from_server_receiver.try_recv() {
                    Ok(datagram) => {
                        // Convert the datagram to bytes and add it to the link's receive queue
                        let payload = Bytes::copy_from_slice(datagram.payload().as_ref());
                        link.recv.push(payload, time.elapsed());
                    },
                    Err(TryRecvError::Empty) => {
                        // No more messages to process
                        break;
                    },
                    Err(TryRecvError::Disconnected) => {
                        error!("WebTransport connection disconnected");
                        break;
                    }
                }
            }
        });
    }
}

impl Plugin for ClientWebTransportPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, Self::receive.in_set(LinkSet::Receive));
        app.add_systems(PreUpdate, Self::send.in_set(LinkSet::Send));
    }
}
