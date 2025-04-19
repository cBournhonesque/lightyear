#![cfg(target_family = "wasm")]
//! WebTransport client implementation for WASM targets.
use std::net::SocketAddr;
use std::rc::Rc;

use bevy::prelude::*;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tracing::{debug, error, info, trace};
use xwt_core::prelude::*;

use lightyear_connection::client::Disconnected;
use lightyear_connection::id::PeerId;
use lightyear_link::{Link, LinkSet};

#[derive(Component)]
pub struct ClientWebTransportIo {
    local_addr: SocketAddr,
    server_addr: SocketAddr,
    certificate_digest: String,
    buffer: bytes::BytesMut,
    to_server_sender: Option<mpsc::UnboundedSender<Box<[u8]>>>,
    from_server_receiver: Option<mpsc::UnboundedReceiver<Vec<u8>>>,
}

impl ClientWebTransportIo {
    pub fn new(local_addr: SocketAddr, server_addr: SocketAddr, certificate_digest: String) -> Self {
        Self {
            local_addr,
            server_addr,
            certificate_digest,
            buffer: bytes::BytesMut::with_capacity(1472), // MTU
            to_server_sender: None,
            from_server_receiver: None,
        }
    }

    // Adapted from https://github.com/briansmith/ring/blob/befdc87ac7cbca615ab5d68724f4355434d3a620/src/test.rs#L364-L393
    fn from_hex(hex_str: &str) -> core::result::Result<Vec<u8>, String> {
        if hex_str.len() % 2 != 0 {
            return Err(format!(
                "Hex string does not have an even number of digits. Length: {}. String: .{}.",
                hex_str.len(),
                hex_str
            ));
        }

        let mut result = Vec::with_capacity(hex_str.len() / 2);
        for digits in hex_str.as_bytes().chunks(2) {
            let hi = Self::from_hex_digit(digits[0])?;
            let lo = Self::from_hex_digit(digits[1])?;
            result.push((hi * 0x10) | lo);
        }
        Ok(result)
    }

    fn from_hex_digit(d: u8) -> core::result::Result<u8, String> {
        use core::ops::RangeInclusive;
        const DECIMAL: (u8, RangeInclusive<u8>) = (0, b'0'..=b'9');
        const HEX_LOWER: (u8, RangeInclusive<u8>) = (10, b'a'..=b'f');
        const HEX_UPPER: (u8, RangeInclusive<u8>) = (10, b'A'..=b'F');
        for (offset, range) in &[DECIMAL, HEX_LOWER, HEX_UPPER] {
            if range.contains(&d) {
                return Ok(d - range.start() + offset);
            }
        }
        Err(format!("Invalid hex digit '{}'", d as char))
    }
    
    pub fn start_connection(&mut self) -> Result<(), String> {
        // TODO: This can exhaust all available memory unless there is some other way to limit the amount of in-flight data in place
        let (to_server_sender, to_server_receiver) = mpsc::unbounded_channel();
        let (from_server_sender, from_server_receiver) = mpsc::unbounded_channel();
        
        self.to_server_sender = Some(to_server_sender);
        self.from_server_receiver = Some(from_server_receiver);

        let server_url = format!("https://{}", self.server_addr);
        info!("Starting client webtransport task with server url: {}", &server_url);

        let options = xwt_web::WebTransportOptions {
            server_certificate_hashes: vec![xwt_web::CertificateHash {
                algorithm: xwt_web::HashAlgorithm::Sha256,
                value: Self::from_hex(&self.certificate_digest).map_err(|e| e.to_string())?,
            }],
            ..Default::default()
        };
        let endpoint = xwt_web::Endpoint {
            options: options.to_js(),
        };

        let (send, recv) = tokio::sync::oneshot::channel();
        let (send2, recv2) = tokio::sync::oneshot::channel();
        let mut to_server_receiver = to_server_receiver;
        let from_server_sender = from_server_sender;

        wasm_bindgen_futures::spawn_local(async move {
            info!("Starting webtransport io thread");

            let connecting = match endpoint.connect(&server_url).await {
                Ok(e) => e,
                Err(e) => {
                    error!("Error creating webtransport connection: {:?}", e);
                    return;
                }
            };
            let connection = match connecting.wait_connect().await {
                Ok(c) => c,
                Err(e) => {
                    error!("Error connecting to server: {:?}", e);
                    return;
                }
            };
            
            let connection = Rc::new(connection);
            send.send(connection.clone()).unwrap();
            send2.send(connection.clone()).unwrap();
        });

        // Separate future for receiving datagrams
        wasm_bindgen_futures::spawn_local(async move {
            let Ok(connection) = recv.await else {
                return;
            };
            loop {
                tokio::select! {
                    _ = connection.transport.closed() => return,
                    to_send = connection.receive_datagram() => {
                        match to_send {
                           Ok(data) => {
                               trace!("receive datagram from server: {:?}", &data);
                               from_server_sender.send(data).unwrap_or_else(|e| {
                                   error!("Error sending received data to channel: {:?}", e);
                               });
                           }
                           Err(e) => {
                               error!("receive_datagram connection error: {:?}", e);
                           }
                        }
                    }
                }
            }
        });

        // Separate future for sending datagrams
        wasm_bindgen_futures::spawn_local(async move {
            let Ok(connection) = recv2.await else {
                return;
            };
            loop {
                tokio::select! {
                    _ = connection.transport.closed() => return,
                    recv = to_server_receiver.recv() => {
                        if let Some(msg) = recv {
                            trace!("send datagram to server: {:?}", &msg);
                            connection.send_datagram(msg).await.unwrap_or_else(|e| {
                                error!("send_datagram error: {:?}", e);
                            });
                        }
                    }
                }
            }
        });

        Ok(())
    }
}

pub struct ClientWebTransportPlugin;

impl Plugin for ClientWebTransportPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, Self::send.in_set(LinkSet::Send));
        app.add_systems(PostUpdate, Self::receive.in_set(LinkSet::Receive));
    }
}

impl ClientWebTransportPlugin {
    fn send(
        mut client_query: Query<(&mut ClientWebTransportIo, &Link), Without<Disconnected>>,
    ) {
        client_query.par_iter_mut().for_each(|(mut client_io, link)| {
            if let Some(to_server_sender) = &mut client_io.to_server_sender {
                link.send.drain(..).for_each(|send_payload| {
                    let data = send_payload.as_ref().to_vec().into_boxed_slice();
                    to_server_sender.send(data).unwrap_or_else(|e| {
                        error!("Error sending data to webtransport: {:?}", e);
                    });
                });
            }
        });
    }

    fn receive(
        time: Res<Time<Real>>,
        mut client_query: Query<(&mut ClientWebTransportIo, &mut Link)>,
    ) {
        client_query.par_iter_mut().for_each(|(mut client_io, mut link)| {
            if let Some(from_server_receiver) = &mut client_io.from_server_receiver {
                while let Ok(datagram) = from_server_receiver.try_recv() {
                    client_io.buffer.clear();
                    client_io.buffer.extend_from_slice(&datagram);
                    let payload = client_io.buffer.split().freeze();
                    link.recv.push(payload, time.elapsed());
                }
            }
        });
    }
}
