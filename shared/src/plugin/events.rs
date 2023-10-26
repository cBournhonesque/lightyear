use crate::connection::events::EventContext;
use crate::netcode::ClientId;
use crate::{ChannelKind, Message, Protocol};
use bevy::prelude::{App, Component, Entity, Event};
use std::collections::HashMap;

// pub struct NetworkEvent<Inner: EventContext, Ctx: EventContext> {
//     inner: Inner,
//     context: Ctx,
// }
//
// pub type MessageEvent<Ctx = ()> = NetworkEvent<Message, Ctx>;

#[derive(Event)]
pub struct ConnectEvent<Ctx = ()>(Ctx);

impl<Ctx> ConnectEvent<Ctx> {
    pub fn new(context: Ctx) -> Self {
        Self(context)
    }
    pub fn context(&self) -> &Ctx {
        &self.0
    }
}

#[derive(Event)]
pub struct DisconnectEvent<Ctx = ()>(Ctx);

impl<Ctx> DisconnectEvent<Ctx> {
    pub fn new(context: Ctx) -> Self {
        Self(context)
    }
    pub fn context(&self) -> &Ctx {
        &self.0
    }
}
// TODO: for server
#[derive(Event)]
pub struct MessageEvent<M: Message, Ctx = ()> {
    message: M,
    context: Ctx,
}

impl<M: Message, Ctx> MessageEvent<M, Ctx> {
    pub fn new(message: M, context: Ctx) -> Self {
        Self { message, context }
    }

    pub fn message(&self) -> &M {
        &self.message
    }

    pub fn context(&self) -> &Ctx {
        &self.context
    }
}

#[derive(Event)]
/// Event emitted on server every time a SpawnEntity replication message gets sent to a client
// TODO: should we change this to when it is received?
pub struct EntitySpawnEvent<Ctx = ()> {
    entity: Entity,
    context: Ctx,
}

impl<Ctx> EntitySpawnEvent<Ctx> {
    pub fn new(entity: Entity, context: Ctx) -> Self {
        Self { entity, context }
    }

    pub fn entity(&self) -> &Entity {
        &self.entity
    }

    pub fn context(&self) -> &Ctx {
        &self.context
    }
}

#[derive(Event)]
pub struct DespawnEntityEvent(pub ClientId, pub Entity);

#[derive(Event)]
pub struct InsertComponentEvent<C: Component> {
    inner: Vec<(ClientId, Entity)>,
    marker: std::marker::PhantomData<C>,
}

#[derive(Event)]
pub struct RemoveComponentEvent<C: Component> {
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
