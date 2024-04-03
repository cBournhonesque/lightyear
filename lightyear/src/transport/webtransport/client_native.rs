#![cfg(not(target_family = "wasm"))]
//! WebTransport client implementation.
use crate::transport::error::{Error, Result};
use crate::transport::{PacketReceiver, PacketSender, Transport, MTU};
use anyhow::Context;
use async_compat::Compat;
use bevy::tasks::{IoTaskPool, TaskPool};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tracing::{debug, error, info, trace};

use wtransport;
use wtransport::datagram::Datagram;
use wtransport::ClientConfig;

/// WebTransport client socket
pub struct WebTransportClientSocket {
    client_addr: SocketAddr,
    server_addr: SocketAddr,
    sender: Option<WebTransportClientPacketSender>,
    receiver: Option<WebTransportClientPacketReceiver>,
}

impl WebTransportClientSocket {
    pub fn new(client_addr: SocketAddr, server_addr: SocketAddr) -> Self {
        Self {
            client_addr,
            server_addr,
            sender: None,
            receiver: None,
        }
    }
}

impl Transport for WebTransportClientSocket {
    fn local_addr(&self) -> SocketAddr {
        self.client_addr
    }

    fn connect(&mut self) -> Result<()> {
        let client_addr = self.client_addr;
        let server_addr = self.server_addr;
        let (to_server_sender, mut to_server_receiver) = mpsc::unbounded_channel();
        let (from_server_sender, from_server_receiver) = mpsc::unbounded_channel();

        let config = ClientConfig::builder()
            .with_bind_address(client_addr)
            .with_no_cert_validation()
            .build();
        let server_url = format!("https://{}", server_addr);
        debug!(
            "Starting client webtransport task with server url: {}",
            &server_url
        );

        // &Scope<Result<>>
        let connection = IoTaskPool::get()
            .scope(
                |scope: &bevy::tasks::Scope<'_, '_, Result<wtransport::Connection>>| {
                    // native wtransport must run in a tokio runtime; we use async-compat instead
                    scope.spawn(Compat::new(async move {
                        let endpoint = wtransport::Endpoint::client(config).map_err(Error::from)?;
                        let connection =
                            endpoint.connect(&server_url).await.map_err(Error::from)?;
                        Ok(connection)
                    }))
                },
            )
            .pop()
            .unwrap()?;

        let connection = Arc::new(connection);

        // NOTE (IMPORTANT!):
        // - we spawn two different futures for receive and send datagrams
        // - if we spawned only one future and used tokio::select!(), the branch that is not selected would be cancelled
        // - this means that we might recreate a new future in `connection.receive_datagram()` instead of just continuing
        //   to poll the existing one. This is FAULTY behaviour
        // - if you want to use tokio::Select, you have to first pin the Future, and then select on &mut Future. Only the reference gets
        //   cancelled
        let connection_recv = connection.clone();
        let recv_handle = IoTaskPool::get().spawn(async move {
            loop {
                match connection_recv.receive_datagram().await {
                    Ok(data) => {
                        trace!("receive datagram from server: {:?}", &data);
                        from_server_sender.send(data).unwrap();
                    }
                    Err(e) => {
                        error!("receive_datagram connection error: {:?}", e);
                    }
                }
            }
        });
        let connection_send = connection.clone();
        let send_handle = IoTaskPool::get().spawn(async move {
            loop {
                if let Some(msg) = to_server_receiver.recv().await {
                    trace!("send datagram to server: {:?}", &msg);
                    connection_send.send_datagram(msg).unwrap_or_else(|e| {
                        error!("send_datagram error: {:?}", e);
                    });
                }
            }
        });
        IoTaskPool::get()
            .spawn(async move {
                connection.closed().await;
                info!("WebTransport connection closed.");
                recv_handle.cancel().await;
                send_handle.cancel().await;
            })
            .detach();

        self.sender = Some(WebTransportClientPacketSender { to_server_sender });
        self.receiver = Some(WebTransportClientPacketReceiver {
            server_addr,
            from_server_receiver,
            buffer: [0; MTU],
        });
        Ok(())
    }

    fn split(&mut self) -> (&mut dyn PacketSender, &mut dyn PacketReceiver) {
        (
            self.sender.as_mut().unwrap(),
            self.receiver.as_mut().unwrap(),
        )
    }

    // fn split(&mut self) -> (&mut Box<dyn PacketSender>, &mut Box<dyn PacketReceiver>) {
    //     (
    //         &mut Box::new(self.sender.as_mut()),
    //         &mut Box::new(self.receiver.as_mut()),
    //     )
    // }
}

struct WebTransportClientPacketSender {
    to_server_sender: mpsc::UnboundedSender<Box<[u8]>>,
}

impl PacketSender for WebTransportClientPacketSender {
    fn send(&mut self, payload: &[u8], address: &SocketAddr) -> Result<()> {
        let data = payload.to_vec().into_boxed_slice();
        self.to_server_sender
            .send(data)
            .map_err(|e| std::io::Error::other(format!("send_datagram error: {:?}", e)).into())
    }
}

struct WebTransportClientPacketReceiver {
    server_addr: SocketAddr,
    from_server_receiver: mpsc::UnboundedReceiver<Datagram>,
    buffer: [u8; MTU],
}

impl PacketReceiver for WebTransportClientPacketReceiver {
    fn recv(&mut self) -> Result<Option<(&mut [u8], SocketAddr)>> {
        match self.from_server_receiver.try_recv() {
            Ok(data) => {
                // convert from datagram to payload via xwt
                self.buffer[..data.len()].copy_from_slice(data.payload().as_ref());
                Ok(Some((&mut self.buffer[..data.len()], self.server_addr)))
            }
            Err(e) => {
                if e == TryRecvError::Empty {
                    Ok(None)
                } else {
                    Err(std::io::Error::other(format!("receive_datagram error: {:?}", e)).into())
                }
            }
        }
    }
}
