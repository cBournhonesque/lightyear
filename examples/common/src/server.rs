//! This module introduces a settings struct that can be used to configure the server and client.
#![allow(unused_imports)]
#![allow(unused_variables)]
use std::net::{Ipv4Addr, SocketAddr};

use bevy::asset::ron;
use bevy::prelude::*;
use core::time::Duration;

use crate::shared::SharedSettings;
#[cfg(not(target_family = "wasm"))]
use async_compat::Compat;
use bevy::ecs::component::HookContext;
use bevy::ecs::world::DeferredWorld;
#[cfg(not(target_family = "wasm"))]
use bevy::tasks::IoTaskPool;
use lightyear::netcode::{NetcodeServer, PRIVATE_KEY_BYTES};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::webtransport::wtransport::Identity;
use serde::{Deserialize, Serialize};
use tracing::warn;

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
pub fn get_digest_on_wasm() -> Option<String> {
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


#[derive(Component, Debug)]
#[component(on_add = ExampleServer::on_add)]
pub struct ExampleServer {
    /// Possibly add a conditioner to simulate network conditions
    pub conditioner: Option<RecvLinkConditioner>,
    /// Which transport to use
    pub transport: ServerTransports,
    pub shared: SharedSettings,
}

impl ExampleServer {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        let entity = context.entity;
        world.commands().queue(move |world: &mut World| -> Result {
            let mut entity_mut = world.entity_mut(entity);
            let settings = entity_mut.take::<ExampleServer>().unwrap();
            entity_mut.insert((
                Server::default(),
                Name::from("Server"),
            ));

            if cfg!(feature = "netcode") {
                // Use private key from environment variable, if set. Otherwise from settings file.
                let private_key = if let Some(key) = parse_private_key_from_env() {
                    info!("Using private key from LIGHTYEAR_PRIVATE_KEY env var");
                    key
                } else {
                    settings.shared.private_key
                };
                entity_mut.insert(NetcodeServer::new(NetcodeConfig {
                    protocol_id: settings.shared.protocol_id,
                    private_key,
                    ..Default::default()
                }));
            }

            match settings.transport {
                #[cfg(feature = "udp")]
                ServerTransports::Udp { local_port } => {
                    let server_addr = SocketAddr::new(
                        Ipv4Addr::UNSPECIFIED.into(),
                        local_port,
                    );
                    entity_mut.insert(ServerUdpIo::new(server_addr));
                }
                ServerTransports::WebTransport { local_port, certificate} => {
                    let server_addr = SocketAddr::new(
                        Ipv4Addr::UNSPECIFIED.into(),
                        local_port,
                    );
                    entity_mut.insert(WebTransportServer {
                        server_addr,
                        certificate: (&certificate).into()
                    });
                }
                _ => {}
            };
            Ok(())
        });
    }

}


#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

impl From<&WebTransportCertificateSettings> for Identity {
    fn from(wt: &WebTransportCertificateSettings) -> Identity {
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
                let identity = Identity::self_signed(sans).unwrap();
                let digest = identity.certificate_chain().as_slice()[0].hash();
                println!("🔐 Certificate digest: {digest}");
                identity
            }
            WebTransportCertificateSettings::FromFile {
                cert: cert_pem_path,
                key: private_key_pem_path,
            } => {
                println!(
                    "Reading certificate PEM files:\n * cert: {}\n * key: {}",
                    cert_pem_path, private_key_pem_path
                );
                // this is async because we need to load the certificate from io
                // we need async_compat because wtransport expects a tokio reactor
                let identity = IoTaskPool::get()
                    .scope(|s| {
                        s.spawn(Compat::new(async {
                            Identity::load_pemfiles(cert_pem_path, private_key_pem_path)
                                .await
                                .unwrap()
                        }));
                    })
                    .pop()
                    .unwrap();
                let digest = identity.certificate_chain().as_slice()[0].hash();
                println!("🔐 Certificate digest: {digest}");
                identity
            }
        }
    }
}



/// Reads and parses the LIGHTYEAR_PRIVATE_KEY environment variable into a private key.
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

