//! This module parses the settings.ron file and builds a lightyear configuration from it
use bevy::utils::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use async_compat::Compat;
use bevy::tasks::IoTaskPool;
use serde::{Deserialize, Serialize};

use lightyear::prelude::client::{Authentication, SteamConfig};
use lightyear::prelude::{ClientId, IoConfig, LinkConditionerConfig, TransportConfig};

use crate::server::Certificate;
use crate::{client, server};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ClientTransports {
    #[cfg(not(target_family = "wasm"))]
    Udp,
    WebTransport {
        certificate_digest: String,
    },
    WebSocket,
    Steam {
        app_id: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ServerTransports {
    Udp {
        local_port: u16,
    },
    WebTransport {
        local_port: u16,
    },
    WebSocket {
        local_port: u16,
    },
    Steam {
        app_id: u32,
        server_ip: Ipv4Addr,
        game_port: u16,
        query_port: u16,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Conditioner {
    /// One way latency in milliseconds
    pub(crate) latency_ms: u16,
    /// One way jitter in milliseconds
    pub(crate) jitter_ms: u16,
    /// Percentage of packet loss
    pub(crate) packet_loss: f32,
}

impl Conditioner {
    pub fn build(&self) -> LinkConditionerConfig {
        LinkConditionerConfig {
            incoming_latency: std::time::Duration::from_millis(self.latency_ms as u64),
            incoming_jitter: std::time::Duration::from_millis(self.jitter_ms as u64),
            incoming_loss: self.packet_loss,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerSettings {
    /// If true, disable any rendering-related plugins
    pub(crate) headless: bool,

    /// If true, enable bevy_inspector_egui
    pub(crate) inspector: bool,

    /// If true, apply prediction to all clients (even other clients)
    pub(crate) predict_all: bool,

    /// Possibly add a conditioner to simulate network conditions
    pub(crate) conditioner: Option<Conditioner>,

    /// Which transport to use
    pub(crate) transport: Vec<ServerTransports>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClientSettings {
    /// If true, enable bevy_inspector_egui
    pub(crate) inspector: bool,

    /// The client id
    pub(crate) client_id: u64,

    /// The client port to listen on
    pub(crate) client_port: u16,

    /// The ip address of the server
    pub(crate) server_addr: Ipv4Addr,

    /// The port of the server
    pub(crate) server_port: u16,

    /// Which transport to use
    pub(crate) transport: ClientTransports,

    /// Possibly add a conditioner to simulate network conditions
    pub(crate) conditioner: Option<Conditioner>,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
pub struct SharedSettings {
    /// An id to identify the protocol version
    pub(crate) protocol_id: u64,

    /// a 32-byte array to authenticate via the Netcode.io protocol
    pub(crate) private_key: [u8; 32],
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Settings {
    pub server: ServerSettings,
    pub client: ClientSettings,
    pub shared: SharedSettings,
}

pub fn build_server_netcode_config(
    conditioner: Option<&Conditioner>,
    shared: &SharedSettings,
    transport_config: TransportConfig,
) -> server::NetConfig {
    let conditioner = conditioner.map_or(None, |c| {
        Some(LinkConditionerConfig {
            incoming_latency: Duration::from_millis(c.latency_ms as u64),
            incoming_jitter: Duration::from_millis(c.jitter_ms as u64),
            incoming_loss: c.packet_loss,
        })
    });
    let netcode_config = server::NetcodeConfig::default()
        .with_protocol_id(shared.protocol_id)
        .with_key(shared.private_key);
    let io_config = IoConfig::from_transport(transport_config);
    let io_config = if let Some(conditioner) = conditioner {
        io_config.with_conditioner(conditioner)
    } else {
        io_config
    };
    server::NetConfig::Netcode {
        config: netcode_config,
        io: io_config,
    }
}

/// Parse the settings into a list of `NetConfig` that are used to configure how the lightyear server
/// listens for incoming client connections
#[cfg(not(target_family = "wasm"))]
pub fn get_server_net_configs(settings: &Settings) -> Vec<server::NetConfig> {
    settings
        .server
        .transport
        .iter()
        .map(|t| match t {
            ServerTransports::Udp { local_port } => crate::build_server_netcode_config(
                settings.server.conditioner.as_ref(),
                &settings.shared,
                TransportConfig::UdpSocket(SocketAddr::new(
                    Ipv4Addr::UNSPECIFIED.into(),
                    *local_port,
                )),
            ),
            ServerTransports::WebTransport { local_port } => {
                // this is async because we need to load the certificate from io
                // we need async_compat because wtransport expects a tokio reactor
                let certificate = IoTaskPool::get()
                    .scope(|s| {
                        s.spawn(Compat::new(async {
                            Certificate::load("../certificates/cert.pem", "../certificates/key.pem")
                                .await
                                .unwrap()
                        }));
                    })
                    .pop()
                    .unwrap();
                let digest = &certificate.hashes()[0].to_string().replace(":", "");
                println!("Generated self-signed certificate with digest: {}", digest);
                crate::build_server_netcode_config(
                    settings.server.conditioner.as_ref(),
                    &settings.shared,
                    TransportConfig::WebTransportServer {
                        server_addr: SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), *local_port),
                        certificate,
                    },
                )
            }
            ServerTransports::WebSocket { local_port } => crate::build_server_netcode_config(
                settings.server.conditioner.as_ref(),
                &settings.shared,
                TransportConfig::WebSocketServer {
                    server_addr: SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), *local_port),
                },
            ),
            ServerTransports::Steam {
                app_id,
                server_ip,
                game_port,
                query_port,
            } => server::NetConfig::Steam {
                config: server::SteamConfig {
                    app_id: *app_id,
                    server_ip: *server_ip,
                    game_port: *game_port,
                    query_port: *query_port,
                    max_clients: 16,
                    version: "1.0".to_string(),
                },
                conditioner: settings
                    .server
                    .conditioner
                    .as_ref()
                    .map_or(None, |c| Some(c.build())),
            },
        })
        .collect()
}

/// Build a netcode config for the client
pub fn build_client_netcode_config(
    client_id: ClientId,
    server_addr: SocketAddr,
    conditioner: Option<&Conditioner>,
    shared: &SharedSettings,
    transport_config: TransportConfig,
) -> client::NetConfig {
    let conditioner = conditioner.map_or(None, |c| Some(c.build()));
    let auth = Authentication::Manual {
        server_addr,
        client_id,
        private_key: shared.private_key,
        protocol_id: shared.protocol_id,
    };
    let netcode_config = client::NetcodeConfig::default();
    let io_config = IoConfig::from_transport(transport_config);
    let io_config = if let Some(conditioner) = conditioner {
        io_config.with_conditioner(conditioner)
    } else {
        io_config
    };
    client::NetConfig::Netcode {
        auth,
        config: netcode_config,
        io: io_config,
    }
}

/// Parse the settings into a `NetConfig` that is used to configure how the lightyear client
/// connects to the server
pub fn get_client_net_config(settings: &Settings, client_id: ClientId) -> client::NetConfig {
    let server_addr = SocketAddr::new(
        settings.client.server_addr.into(),
        settings.client.server_port,
    );
    let client_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), settings.client.client_port);
    match &settings.client.transport {
        #[cfg(not(target_family = "wasm"))]
        ClientTransports::Udp => build_client_netcode_config(
            client_id,
            server_addr,
            settings.client.conditioner.as_ref(),
            &settings.shared,
            TransportConfig::UdpSocket(client_addr),
        ),
        ClientTransports::WebTransport { certificate_digest } => build_client_netcode_config(
            client_id,
            server_addr,
            settings.client.conditioner.as_ref(),
            &settings.shared,
            TransportConfig::WebTransportClient {
                client_addr,
                server_addr,
                #[cfg(target_family = "wasm")]
                certificate_digest,
            },
        ),
        ClientTransports::WebSocket => build_client_netcode_config(
            client_id,
            server_addr,
            settings.client.conditioner.as_ref(),
            &settings.shared,
            TransportConfig::WebSocketClient { server_addr },
        ),
        ClientTransports::Steam { app_id } => client::NetConfig::Steam {
            config: SteamConfig {
                server_addr,
                app_id: *app_id,
            },
            conditioner: settings
                .server
                .conditioner
                .as_ref()
                .map_or(None, |c| Some(c.build())),
        },
    }
}
