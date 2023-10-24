use crate::netcode::ClientId;
use crate::{ChannelKind, Message, Protocol};
use bevy::prelude::{App, Component, Entity, Event};
use std::collections::HashMap;

#[derive(Event)]
pub struct ConnectEvent(pub ClientId);

#[derive(Event)]
pub struct DisconnectEvent(pub ClientId);

// TODO: for server

pub struct MessageEvent<M: Message> {
    inner: HashMap<ChannelKind, Vec<(ClientId, M)>>,
}

#[derive(Event)]
/// Event emitted on server every time a SpawnEntity replication message gets sent to a client
// TODO: should we change this to when it is received?
pub struct SpawnEntityEvent(pub ClientId, pub Entity);

#[derive(Event)]
pub struct DespawnEntityEvent(pub ClientId, pub Entity);

#[derive(Event)]
pub struct InsertComponentEvent<C: Component> {
    inner: Vec<(ClientId, Entity)>,
    marker: std::marker::PhantomData<C>,
}

#[derive(Event)]
pub struct UpdateComponentEvent<C: Component> {
    inner: Vec<(ClientId, Entity)>,
    marker: std::marker::PhantomData<C>,
}

// pub fn add_message_event_systems<M: Message, P: Protocol>(app: &mut App) {
//     app.add_event::<MessageEvent<M>>()
// }
