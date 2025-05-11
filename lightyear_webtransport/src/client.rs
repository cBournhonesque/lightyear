/*! WebTransport client implementation */
use async_compat::Compat;
use bevy::ecs::query::QueryEntityError;
use bevy::prelude::*;
use bevy::tasks::IoTaskPool;
use bytes::{Bytes, BytesMut};
use core::net::SocketAddr;
use lightyear_connection::client::Disconnected;
use lightyear_link::{Link, LinkSet, LinkStart, SendPayload, Unlink, Unlinked};
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
    to_server_sender: Option<mpsc::UnboundedSender<SendPayload>>,
    from_server_receiver: Option<mpsc::UnboundedReceiver<Datagram>>,
    close_sender: Option<async_channel::Sender<()>>,
}

impl ClientWebTransportIo {
    pub fn new(local_addr: SocketAddr, server_addr: SocketAddr) -> Result<Self, std::io::Error> {
        Ok(ClientWebTransportIo {
            local_addr,
            server_addr,
            to_server_sender: None,
            from_server_receiver: None,
            close_sender: None,
        })
    }
}

pub struct ClientWebTransportPlugin;

impl ClientWebTransportPlugin {
    fn link(
        trigger: Trigger<LinkStart>,
        mut query: Query<&mut ClientWebTransportIo>,
    ) {
        if let Ok(mut client_io) = query.get_mut(trigger.target()) {
            let (to_server_sender, mut to_server_receiver) = mpsc::unbounded_channel::<SendPayload>();
            let (from_server_sender, from_server_receiver) = mpsc::unbounded_channel();
            // channels used to cancel the task
            let (close_tx, close_rx) = async_channel::bounded(1);
            client_io.close_sender = Some(close_tx);
            client_io.to_server_sender = Some(to_server_sender);
            client_io.from_server_receiver = Some(from_server_receiver);
            let server_addr = client_io.server_addr;
            let local_addr = client_io.local_addr;
            
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
                                // TODO: bubble this and trigger an Unlinked ?
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

        }
    }

    fn unlink(
        trigger: Trigger<Unlink>,
        mut query: Query<&mut ClientWebTransportIo>,
    ) -> Result {
        if let Ok(mut client_io) = query.get_mut(trigger.target()) {
            if let Some(close_sender) = client_io.close_sender.take() {
                // Send a close signal to the task
                if let Err(e) = close_sender.send_blocking(()) {
                    error!("Failed to send close signal: {:?}", e);
                }
            }
            client_io.close_sender = None;
            client_io.to_server_sender = None;
            client_io.from_server_receiver = None;
        }
        Ok(())
    }

    fn send(
        mut client_query: Query<(&mut ClientWebTransportIo, &mut Link), Without<Unlinked>>
    ) {
        client_query.par_iter_mut().for_each(|(mut client_io, mut link)| {
            link.send.drain().for_each(|send_payload| {
                client_io.to_server_sender.as_mut().map(|s| s.send(send_payload).unwrap_or_else(|e| {
                    error!("Error sending WebTransport packet: {}", e);
                }));
            });
        });
    }

    fn receive(
        time: Res<Time<Real>>,
        mut client_query: Query<(&mut ClientWebTransportIo, &mut Link), Without<Unlinked>>,
    ) {
        client_query.par_iter_mut().for_each(|(mut client_io, mut link)| {
            // Process all available messages from the WebTransport connection
            let Some(receiver) = client_io.from_server_receiver.as_mut() else {
                return
            };
            loop {
                match receiver.try_recv() {
                    Ok(datagram) => {
                        // Convert the datagram to bytes and add it to the link's receive queue
                        link.recv.push(datagram.payload(), time.elapsed());
                    }
                    Err(TryRecvError::Empty) => {
                        // No more messages to process
                        break;
                    }
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
        app.add_observer(Self::link);
        app.add_observer(Self::unlink);
        app.add_systems(PreUpdate, Self::receive.in_set(LinkSet::Receive));
        app.add_systems(PreUpdate, Self::send.in_set(LinkSet::Send));
    }
}
