//! This module parses the settings.ron file and builds a lightyear configuration from it
#![allow(unused_imports)]
#![allow(unused_variables)]
use std::net::{Ipv4Addr, SocketAddr};

use bevy::asset::ron;
use bevy::prelude::*;
use bevy::utils::Duration;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[cfg(not(target_family = "wasm"))]
use async_compat::Compat;
#[cfg(not(target_family = "wasm"))]
use bevy::tasks::IoTaskPool;

use lightyear::connection::netcode::PRIVATE_KEY_BYTES;
use lightyear::prelude::client::Authentication;
#[cfg(feature = "steam")]
use lightyear::prelude::client::{SocketConfig, SteamConfig};
use lightyear::prelude::{CompressionConfig, LinkConditionerConfig};

use lightyear::prelude::{client, server};

/// We parse the settings.ron file to read the settings
pub fn read_settings<T: DeserializeOwned>(settings_str: &str) -> T {
    ron::de::from_str::<T>(settings_str).expect("Could not deserialize the settings file")
}

/// Read certificate digest from alternate sources, for WASM builds.
#[cfg(target_family = "wasm")]
#[allow(unreachable_patterns)]
pub fn modify_digest_on_wasm(client_settings: &mut ClientSettings) -> Option<String> {
    if let Some(new_digest) = get_digest_on_wasm() {
        match &client_settings.transport {
            ClientTransports::WebTransport { certificate_digest } => {
                client_settings.transport = ClientTransports::WebTransport {
                    certificate_digest: new_digest.clone(),
                };
                Some(new_digest)
            }
            // This could be unreachable if only WebTransport feature is enabled.
            // hence we supress this warning with the allow directive above.
            _ => None,
        }
    } else {
        None
    }
}

#[cfg(target_family = "wasm")]
fn get_digest_on_wasm() -> Option<String> {
    let window = web_sys::window().expect("expected window");

    if let Ok(obj) = window.location().hash() {
        info!("Using cert digest from window.location().hash()");
        let cd = obj.replace("#", "");
        if cd.len() > 10 {
            // lazy sanity check.
            return Some(cd);
        }
    }

    if let Some(obj) = window.get("CERT_DIGEST") {
        info!("Using cert digest from window.CERT_DIGEST");
        return Some(obj.as_string().expect("CERT_DIGEST should be a string"));
    }

    None
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ClientTransports {
    #[cfg(not(target_family = "wasm"))]
    Udp,
    WebTransport {
        certificate_digest: String,
    },
    #[cfg(feature = "websocket")]
    WebSocket,
    #[cfg(feature = "steam")]
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
        certificate: WebTransportCertificateSettings,
    },
    #[cfg(feature = "websocket")]
    WebSocket {
        local_port: u16,
    },
    #[cfg(feature = "steam")]
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
            incoming_latency: Duration::from_millis(self.latency_ms as u64),
            incoming_jitter: Duration::from_millis(self.jitter_ms as u64),
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

    /// Possibly add a conditioner to simulate network conditions
    pub(crate) conditioner: Option<Conditioner>,

    /// Which transport to use
    pub transport: Vec<ServerTransports>,
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
    pub server_addr: Ipv4Addr,

    /// The port of the server
    pub server_port: u16,

    /// Which transport to use
    pub(crate) transport: ClientTransports,

    /// Possibly add a conditioner to simulate network conditions
    pub(crate) conditioner: Option<Conditioner>,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
pub struct SharedSettings {
    /// An id to identify the protocol version
    pub protocol_id: u64,

    /// a 32-byte array to authenticate via the Netcode.io protocol
    pub private_key: [u8; 32],

    /// compression options
    pub(crate) compression: CompressionConfig,
}

#[derive(Resource, Debug, Clone, Deserialize, Serialize)]
pub struct Settings {
    pub server: ServerSettings,
    pub client: ClientSettings,
    pub shared: SharedSettings,
}

#[cfg(feature = "server")]
pub(crate) fn build_server_netcode_config(
    conditioner: Option<&Conditioner>,
    shared: &SharedSettings,
    transport_config: server::ServerTransport,
) -> server::NetConfig {
    let conditioner = conditioner.map(|c| LinkConditionerConfig {
        incoming_latency: Duration::from_millis(c.latency_ms as u64),
        incoming_jitter: Duration::from_millis(c.jitter_ms as u64),
        incoming_loss: c.packet_loss,
    });
    // Use private key from environment variable, if set. Otherwise from settings file.
    let privkey = if let Some(key) = parse_private_key_from_env() {
        info!("Using private key from LIGHTYEAR_PRIVATE_KEY env var");
        key
    } else {
        shared.private_key
    };

    println!("🔑 Using lightyear private key: {privkey:?}");
    println!("🔑 Using lightyear protocol id:: {}", shared.protocol_id);

    let netcode_config = server::NetcodeConfig::default()
        .with_protocol_id(shared.protocol_id)
        .with_key(privkey);
    let io_config = server::IoConfig {
        transport: transport_config,
        conditioner,
        compression: shared.compression,
    };
    server::NetConfig::Netcode {
        config: netcode_config,
        io: io_config,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub enum WebTransportCertificateSettings {
    /// Generate a self-signed certificate, with given SANs list to add to the certifictate
    /// eg: ["example.com", "*.gameserver.example.org", "10.1.2.3", "::1"]
    AutoSelfSigned(Vec<String>),
    /// Load certificate pem files from disk
    FromFile {
        /// Path to cert .pem file
        cert: String,
        /// Path to private key .pem file
        key: String,
    },
}

impl Default for WebTransportCertificateSettings {
    fn default() -> Self {
        let sans = vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
        ];
        WebTransportCertificateSettings::AutoSelfSigned(sans)
    }
}

#[cfg(feature = "server")]
impl From<&WebTransportCertificateSettings> for server::Identity {
    fn from(wt: &WebTransportCertificateSettings) -> server::Identity {
        match wt {
            WebTransportCertificateSettings::AutoSelfSigned(sans) => {
                // In addition to and Subject Alternate Names (SAN) added via the config,
                // we add the public ip and domain for edgegap, if detected, and also
                // any extra values specified via the SELF_SIGNED_SANS environment variable.
                let mut sans = sans.clone();
                // Are we running on edgegap?
                if let Ok(public_ip) = std::env::var("ARBITRIUM_PUBLIC_IP") {
                    println!("🔐 SAN += ARBITRIUM_PUBLIC_IP: {}", public_ip);
                    sans.push(public_ip);
                    sans.push("*.pr.edgegap.net".to_string());
                }
                // generic env to add domains and ips to SAN list:
                // SELF_SIGNED_SANS="example.org,example.com,127.1.1.1"
                if let Ok(san) = std::env::var("SELF_SIGNED_SANS") {
                    println!("🔐 SAN += SELF_SIGNED_SANS: {}", san);
                    sans.extend(san.split(',').map(|s| s.to_string()));
                }
                println!("🔐 Generating self-signed certificate with SANs: {sans:?}");
                let identity = server::Identity::self_signed(sans).unwrap();
                let digest = identity.certificate_chain().as_slice()[0].hash();
                println!("🔐 Certificate digest: {digest}");
                identity
            }
            WebTransportCertificateSettings::FromFile {
                cert: cert_pem_path,
                key: private_key_pem_path,
            } => {
                // this is async because we need to load the certificate from io
                // we need async_compat because wtransport expects a tokio reactor
                let identity = IoTaskPool::get()
                    .scope(|s| {
                        s.spawn(Compat::new(async {
                            server::Identity::load_pemfiles(cert_pem_path, private_key_pem_path)
                                .await
                                .unwrap()
                        }));
                    })
                    .pop()
                    .unwrap();
                println!(
                    "Reading certificate PEM files:\n * cert: {}\n * key: {}",
                    cert_pem_path, private_key_pem_path
                );
                let digest = identity.certificate_chain().as_slice()[0].hash();
                println!("🔐 Certificate digest: {digest}");
                identity
            }
        }
    }
}

/// Parse the settings into a list of `NetConfig` that are used to configure how the lightyear server
/// listens for incoming client connections
#[cfg(feature = "server")]
pub(crate) fn get_server_net_configs(settings: &Settings) -> Vec<server::NetConfig> {
    settings
        .server
        .transport
        .iter()
        .map(|t| match t {
            ServerTransports::Udp { local_port } => build_server_netcode_config(
                settings.server.conditioner.as_ref(),
                &settings.shared,
                server::ServerTransport::UdpSocket(SocketAddr::new(
                    Ipv4Addr::UNSPECIFIED.into(),
                    *local_port,
                )),
            ),
            ServerTransports::WebTransport {
                local_port,
                certificate,
            } => {
                let transport_config = server::ServerTransport::WebTransportServer {
                    server_addr: SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), *local_port),
                    certificate: certificate.into(),
                };
                build_server_netcode_config(
                    settings.server.conditioner.as_ref(),
                    &settings.shared,
                    transport_config,
                )
            }
            // TODO allow enum but filter if support not compiled in with a warning?
            // "Websocket transport configured but 'websocket' feature disabled"
            #[cfg(feature = "websocket")]
            ServerTransports::WebSocket { local_port } => build_server_netcode_config(
                settings.server.conditioner.as_ref(),
                &settings.shared,
                server::ServerTransport::WebSocketServer {
                    server_addr: SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), *local_port),
                },
            ),
            #[cfg(feature = "steam")]
            ServerTransports::Steam {
                app_id,
                server_ip,
                game_port,
                query_port,
            } => server::NetConfig::Steam {
                steamworks_client: None,
                config: server::SteamConfig {
                    app_id: *app_id,
                    socket_config: server::SocketConfig::Ip {
                        server_ip: *server_ip,
                        game_port: *game_port,
                        query_port: *query_port,
                    },
                    max_clients: 16,
                    ..default()
                },
                conditioner: settings.server.conditioner.as_ref().map(|c| c.build()),
            },
        })
        .collect()
}

/// Build a netcode config for the client
pub(crate) fn build_client_netcode_config(
    client_id: u64,
    server_addr: SocketAddr,
    conditioner: Option<&Conditioner>,
    shared: &SharedSettings,
    transport_config: client::ClientTransport,
) -> client::NetConfig {
    let conditioner = conditioner.map(|c| c.build());
    // TODO no point having the private key in shared settings. client's shouldn't know it.
    // use dummy zeroed key explicitly here.
    let auth = Authentication::Manual {
        server_addr,
        client_id,
        private_key: shared.private_key,
        protocol_id: shared.protocol_id,
    };
    println!("Auth: {auth:?}");
    println!("TransportConfig: {transport_config:?}");
    let netcode_config = client::NetcodeConfig::default();
    let io_config = client::IoConfig {
        transport: transport_config,
        conditioner,
        compression: shared.compression,
    };
    client::NetConfig::Netcode {
        auth,
        config: netcode_config,
        io: io_config,
    }
}

/// Parse the settings into a `NetConfig` that is used to configure how the lightyear client
/// connects to the server
pub fn get_client_net_config(settings: &Settings, client_id: u64) -> client::NetConfig {
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
            client::ClientTransport::UdpSocket(client_addr),
        ),
        ClientTransports::WebTransport { certificate_digest } => build_client_netcode_config(
            client_id,
            server_addr,
            settings.client.conditioner.as_ref(),
            &settings.shared,
            client::ClientTransport::WebTransportClient {
                client_addr,
                server_addr,
                #[cfg(target_family = "wasm")]
                certificate_digest: certificate_digest.to_string().replace(":", ""),
            },
        ),
        #[cfg(feature = "websocket")]
        ClientTransports::WebSocket => build_client_netcode_config(
            client_id,
            server_addr,
            settings.client.conditioner.as_ref(),
            &settings.shared,
            client::ClientTransport::WebSocketClient { server_addr },
        ),
        #[cfg(feature = "steam")]
        ClientTransports::Steam { app_id } => client::NetConfig::Steam {
            steamworks_client: None,
            config: SteamConfig {
                socket_config: SocketConfig::Ip { server_addr },
                app_id: *app_id,
            },
            conditioner: settings.server.conditioner.as_ref().map(|c| c.build()),
        },
    }
}

/// Reads and parses the LIGHTYEAR_PRIVATE_KEY environment variable into a private key.
#[cfg(feature = "server")]
pub fn parse_private_key_from_env() -> Option<[u8; PRIVATE_KEY_BYTES]> {
    let Ok(key_str) = std::env::var("LIGHTYEAR_PRIVATE_KEY") else {
        return None;
    };
    let private_key: Vec<u8> = key_str
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',')
        .collect::<String>()
        .split(',')
        .map(|s| {
            s.parse::<u8>()
                .expect("Failed to parse number in private key")
        })
        .collect();

    if private_key.len() != PRIVATE_KEY_BYTES {
        panic!(
            "Private key must contain exactly {} numbers",
            PRIVATE_KEY_BYTES
        );
    }

    let mut bytes = [0u8; PRIVATE_KEY_BYTES];
    bytes.copy_from_slice(&private_key);
    Some(bytes)
}

/// This is the path to the websocket endpoint on `bevygap_matchmaker_httpd``
///
/// * Checks for window.MATCHMAKER_URL global variable (set in index.html)
///
/// otherwise, defaults to transforming the window.location:
///
/// * Changes http://www.example.com/whatever  -> ws://www.example.com/matchmaker/ws
/// * Changes https://www.example.com/whatever -> wss://www.example.com/matchmaker/ws
#[cfg(target_family = "wasm")]
pub fn get_matchmaker_url() -> String {
    const MATCHMAKER_PATH: &str = "/matchmaker/ws";
    let window = web_sys::window().expect("expected window");
    if let Some(obj) = window.get("MATCHMAKER_URL") {
        info!("Using matchmaker url from window.MATCHMAKER_URL");
        obj.as_string().expect("MATCHMAKER_URL should be a string")
    } else {
        info!("Creating matchmaker url from window.location");
        let location = window.location();
        let host = location.host().expect("Expected host");
        let proto = if location.protocol().expect("Expected protocol") == "https:" {
            "wss:"
        } else {
            "ws:"
        };
        format!("{proto}//{host}{MATCHMAKER_PATH}")
    }
}

/// This is the path to the websocket endpoint on `bevygap_matchmaker_httpd``
///
/// * Reads COMPILE_TIME_MATCHMAKER_URL environment variable during compilation
///   otherwise:
/// * Reads the MATCHMAKER_URL environment variable at runtime
///   otherwise:
/// * Defaults to a localhost dev url.
#[cfg(not(target_family = "wasm"))]
pub fn get_matchmaker_url() -> String {
    const MATCHMAKER_PATH: &str = "/matchmaker/ws";
    // use compile-time env variable, this overwrites everything if set.
    match option_env!("COMPILE_TIME_MATCHMAKER_URL") {
        Some(url) => {
            info!("Using matchmaker url from COMPILE_TIME_MATCHMAKER_URL env");
            url.to_string()
        }
        None => {
            if let Ok(url) = std::env::var("MATCHMAKER_URL") {
                info!("Using matchmaker url from MATCHMAKER_URL env");
                url
            } else {
                warn!("Using default localhost dev url for matchmaker");
                format!("ws://127.0.0.1:3000{MATCHMAKER_PATH}")
            }
        }
    }
}
