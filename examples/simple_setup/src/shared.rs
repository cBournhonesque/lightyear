//! This module contains the shared code between the client and the server.

use bevy::prelude::*;
#[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
use bevy::tasks::IoTaskPool;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use core::time::Duration;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

pub const FIXED_TIMESTEP_HZ: f64 = 64.0;

pub const SERVER_REPLICATION_INTERVAL: Duration = Duration::from_millis(100);

pub const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5000);

#[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
pub(crate) fn webtransport_self_signed_certificate() -> Identity {
    // Keep this file-backed so native servers and wasm clients agree on certificates/digest.txt.
    // Runtime-generated self-signed identities have a new digest each run.
    let cert = format!("{}/../../certificates/cert.pem", env!("CARGO_MANIFEST_DIR"));
    let key = format!("{}/../../certificates/key.pem", env!("CARGO_MANIFEST_DIR"));
    IoTaskPool::get()
        .scope(|s| {
            s.spawn(async_compat::Compat::new(async move {
                Identity::load_pemfiles(&cert, &key).await.unwrap()
            }));
        })
        .pop()
        .unwrap()
}

#[derive(Clone)]
pub struct SharedPlugin;

pub struct Channel1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // Register your protocol, which is shared between client and server
        app.register_message::<Message1>()
            .add_direction(NetworkDirection::Bidirectional);

        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);
    }
}
