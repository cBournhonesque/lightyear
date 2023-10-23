use bevy_ecs::prelude::Event;
use lightyear_shared::netcode::ClientId;

#[derive(Event)]
pub struct ConnectEvent(pub ClientId);

#[derive(Event)]
pub struct DisconnectEvent(pub ClientId);
