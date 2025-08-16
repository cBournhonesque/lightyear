//! This module contains the shared code between the client and the server.

use bevy::prelude::*;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use core::time::Duration;
use bevy::ecs::entity::MapEntities;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

pub const FIXED_TIMESTEP_HZ: f64 = 64.0;

pub const SERVER_REPLICATION_INTERVAL: Duration = Duration::from_millis(100);

pub const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 5000);

#[derive(Clone)]
pub struct SharedPlugin;

pub struct Channel1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StressComponent {
    pub(crate) entities: Vec<Entity>,
}

impl MapEntities for StressComponent {
    fn map_entities<E: EntityMapper>(&mut self, entity_mapper: &mut E) {
        self.entities.map_entities(entity_mapper);
    }
}


impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // Register your protocol, which is shared between client and server
        app.add_message::<Message1>()
            .add_direction(NetworkDirection::Bidirectional);
        
        app.register_component::<StressComponent>()
            .add_map_entities();

        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
            .add_direction(NetworkDirection::Bidirectional);
    }
}
