//! Bevy events that will be emitted upon receiving network messages

use std::marker::PhantomData;

use bevy::prelude::{Component, Entity, Event};

#[cfg(feature = "leafwing")]
use crate::inputs::leafwing::InputMessage;
use crate::packet::message::Message;

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

#[cfg(feature = "leafwing")]
#[derive(Event)]
pub(crate) struct InputMessageEvent<A: crate::inputs::leafwing::LeafwingUserAction, Ctx = ()> {
    pub(crate) message: InputMessage<A>,
    pub(crate) context: Ctx,
}

#[cfg(feature = "leafwing")]
impl<A: crate::inputs::leafwing::LeafwingUserAction, Ctx> InputMessageEvent<A, Ctx> {
    pub fn new(message: InputMessage<A>, context: Ctx) -> Self {
        Self { message, context }
    }
    pub fn message(&self) -> &InputMessage<A> {
        &self.message
    }
    pub fn context(&self) -> &Ctx {
        &self.context
    }
}

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
/// Event emitted on server every time we receive an event
pub struct InputEvent<I: crate::inputs::native::UserAction, Ctx = ()> {
    input: Option<I>,
    context: Ctx,
}

impl<I: crate::inputs::native::UserAction, Ctx> InputEvent<I, Ctx> {
    pub fn new(input: Option<I>, context: Ctx) -> Self {
        Self { input, context }
    }

    pub fn input(&self) -> &Option<I> {
        &self.input
    }

    pub fn context(&self) -> &Ctx {
        &self.context
    }
}

#[derive(Event)]
/// Event emitted whenever an replicated entity gets received
pub struct EntitySpawnEvent<Ctx = ()> {
    entity: Entity,
    context: Ctx,
}

impl<Ctx> EntitySpawnEvent<Ctx> {
    pub fn new(entity: Entity, context: Ctx) -> Self {
        Self { entity, context }
    }

    pub fn entity(&self) -> Entity {
        self.entity
    }

    pub fn context(&self) -> &Ctx {
        &self.context
    }
}

#[derive(Event)]
pub struct EntityDespawnEvent<Ctx = ()> {
    entity: Entity,
    context: Ctx,
}

impl<Ctx> EntityDespawnEvent<Ctx> {
    pub fn new(entity: Entity, context: Ctx) -> Self {
        Self { entity, context }
    }

    pub fn entity(&self) -> Entity {
        self.entity
    }

    pub fn context(&self) -> &Ctx {
        &self.context
    }
}

#[derive(Event)]
pub struct ComponentUpdateEvent<C: Component, Ctx = ()> {
    entity: Entity,
    context: Ctx,

    _marker: PhantomData<C>,
}

impl<C: Component, Ctx> ComponentUpdateEvent<C, Ctx> {
    pub fn new(entity: Entity, context: Ctx) -> Self {
        Self {
            entity,
            context,
            _marker: PhantomData,
        }
    }

    pub fn entity(&self) -> Entity {
        self.entity
    }

    pub fn context(&self) -> &Ctx {
        &self.context
    }
}

#[derive(Event, Debug)]
pub struct ComponentInsertEvent<C: Component, Ctx = ()> {
    entity: Entity,
    context: Ctx,

    _marker: PhantomData<C>,
}

impl<C: Component, Ctx> ComponentInsertEvent<C, Ctx> {
    pub fn new(entity: Entity, context: Ctx) -> Self {
        Self {
            entity,
            context,
            _marker: PhantomData,
        }
    }

    pub fn entity(&self) -> Entity {
        self.entity
    }

    pub fn context(&self) -> &Ctx {
        &self.context
    }
}

#[derive(Event)]
pub struct ComponentRemoveEvent<C: Component, Ctx = ()> {
    entity: Entity,
    context: Ctx,

    _marker: PhantomData<C>,
}

impl<C: Component, Ctx> ComponentRemoveEvent<C, Ctx> {
    pub fn new(entity: Entity, context: Ctx) -> Self {
        Self {
            entity,
            context,
            _marker: PhantomData,
        }
    }

    pub fn entity(&self) -> Entity {
        self.entity
    }

    pub fn context(&self) -> &Ctx {
        &self.context
    }
}
