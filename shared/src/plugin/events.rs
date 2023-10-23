use crate::netcode::ClientId;
use bevy_ecs::prelude::Event;

#[derive(Event)]
pub struct ConnectEvent(pub ClientId);

#[derive(Event)]
pub struct DisconnectEvent(pub ClientId);
