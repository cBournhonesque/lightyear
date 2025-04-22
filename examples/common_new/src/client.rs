//! This module introduces a settings struct that can be used to configure the server and client.
#![allow(unused_imports)]
#![allow(unused_variables)]
use std::net::{Ipv4Addr, SocketAddr};

use bevy::asset::ron;
use bevy::prelude::*;
use core::time::Duration;

#[cfg(not(target_family = "wasm"))]
use async_compat::Compat;
use bevy::ecs::component::HookContext;
use bevy::ecs::world::DeferredWorld;
#[cfg(not(target_family = "wasm"))]
use bevy::tasks::IoTaskPool;

use crate::shared::SharedSettings;
use lightyear::input::prelude::InputBuffer;
use lightyear::netcode::client_plugin::NetcodeConfig;
use lightyear::netcode::{NetcodeClient, PRIVATE_KEY_BYTES};
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use tracing::warn;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClientTransports {
    #[cfg(not(target_family = "wasm"))]
    Udp,
    WebTransport {
        #[cfg(target_family = "wasm")]
        certificate_digest: String,
    },
    #[cfg(feature = "websocket")]
    WebSocket,
    #[cfg(feature = "steam")]
    Steam { app_id: u32 },
}

/// Event that examples can trigger to spawn a client.
#[derive(Component, Clone, Debug)]
#[component(on_add = ExampleClient::on_add)]
pub struct ExampleClient {
    pub client_id: u64,
    /// The client port to listen on
    pub client_port: u16,
    /// The socket address of the server
    pub server_addr: SocketAddr,
    /// Possibly add a conditioner to simulate network conditions
    pub conditioner: Option<RecvLinkConditioner>,
    /// Which transport to use
    pub transport: ClientTransports,
    pub shared: SharedSettings
}

impl ExampleClient {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        let entity = context.entity;
        world.commands().queue(move |world: &mut World| -> Result {
            let mut entity_mut = world.entity_mut(entity);
            let settings = entity_mut.take::<ExampleClient>().unwrap();
            let client_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), settings.client_port);
            entity_mut.insert((
                Client::default(),
                Link::new(settings.server_addr, settings.conditioner.clone()),
                ReplicationSender::default(),
                ReplicationReceiver::default(),
                PredictionManager::default(),
            ));

            if cfg!(feature = "netcode") {
                // use dummy zeroed key explicitly here.
                let auth = Authentication::Manual {
                    server_addr: settings.server_addr,
                    client_id: settings.client_id,
                    private_key: settings.shared.private_key,
                    protocol_id: settings.shared.protocol_id,
                };
                let netcode_config = NetcodeConfig {
                    // Make sure that the server times out clients when their connection is closed
                    client_timeout_secs: 3,
                    ..default()
                };
                entity_mut.insert(NetcodeClient::new(auth, netcode_config)?);
            }

            match settings.transport {
                #[cfg(not(target_family = "wasm"))]
                ClientTransports::Udp => {
                    entity_mut.insert(UdpIo::new(client_addr)?);
                }
                _ => {}
                // ClientTransports::WebTransport {
                //     #[cfg(target_family = "wasm")]
                //     certificate_digest,
                // } => {
                //
                // }
            };
            Ok(())
        });
    }
    
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
