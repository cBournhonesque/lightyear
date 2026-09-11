//! This module contains the shared code between the client and the server.

use bevy::prelude::*;
use bevy::prelude::*;
#[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
use bevy::tasks::IoTaskPool;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use core::time::Duration;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

pub const FIXED_TIMESTEP_HZ: f64 = 64.0;

pub const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5000);

#[derive(Clone)]
pub struct SharedPlugin;

pub struct Channel1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

#[derive(
    Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect, Deref, DerefMut, Default,
)]
pub struct PlayerPosition(pub Vec2);

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // PROTOCOL
        // Register a message that can be sent between peers
        app.register_message::<Message1>()
            .add_direction(NetworkDirection::Bidirectional);

        // You can create channels to send messages over. Channels are used to define the conditions (reliability, ordering, etc.) of the messages sent over them
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);

        app.component::<PlayerPosition>().replicate();

        // RENDERING
        app.add_systems(PostUpdate, draw_boxes);
    }
}

/// System that draws the boxes of the player positions.
/// The components should be replicated from the server to the client
pub(crate) fn draw_boxes(mut gizmos: Gizmos, players: Query<&PlayerPosition>) {
    for position in &players {
        gizmos.rect_2d(
            Isometry2d::from_translation(position.0),
            Vec2::ONE * 50.0,
            Color::WHITE,
        );
    }
}
