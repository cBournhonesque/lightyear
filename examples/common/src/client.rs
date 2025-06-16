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
use lightyear::netcode::client_plugin::NetcodeConfig;
use lightyear::netcode::{NetcodeClient, PRIVATE_KEY_BYTES};
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ClientTransports {
    #[cfg(not(target_family = "wasm"))]
    Udp,
    WebTransport,
    #[cfg(feature = "websocket")]
    WebSocket,
    #[cfg(feature = "steam")]
    Steam {
        app_id: u32,
    },
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
    pub shared: SharedSettings,
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
                Link::new(settings.conditioner.clone()),
                LocalAddr(client_addr),
                PeerAddr(settings.server_addr),
                ReplicationReceiver::default(),
                PredictionManager::default(),
                InterpolationManager::default(),
                Name::from("Client"),
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
                    token_expire_secs: -1,
                    ..default()
                };
                entity_mut.insert(NetcodeClient::new(auth, netcode_config)?);
            }

            match settings.transport {
                #[cfg(not(target_family = "wasm"))]
                ClientTransports::Udp => {
                    entity_mut.insert(UdpIo::default());
                }
                ClientTransports::WebTransport => {
                    let certificate_digest = {
                        #[cfg(target_family = "wasm")]
                        {
                            include_str!("../../certificates/digest.txt").to_string()
                        }
                        #[cfg(not(target_family = "wasm"))]
                        {
                            "".to_string()
                        }
                    };
                    entity_mut.insert(WebTransportClientIo { certificate_digest });
                }
                _ => {}
            };
            Ok(())
        });
    }
}
