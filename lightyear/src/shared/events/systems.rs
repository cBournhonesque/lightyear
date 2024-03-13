use bevy::prelude::{Component, Events, World};

use crate::_reexport::FromType;
use crate::packet::message::Message;
use crate::protocol::{EventContext, Protocol};
use crate::shared::events::components::{
    ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, MessageEvent,
};
use crate::shared::events::connection::{
    IterComponentInsertEvent, IterComponentRemoveEvent, IterComponentUpdateEvent, IterMessageEvent,
};

// TODO: would it be easier to have this be a system?

// TODO: make server events a trait, so we can use the same function for client events and server events
//  maybe we have a wrapper around generic Events
pub fn push_message_events<
    M: Message,
    P: Protocol,
    E: IterMessageEvent<P, Ctx>,
    Ctx: EventContext,
>(
    world: &mut World,
    events: &mut E,
) where
    P::Message: TryInto<M, Error = ()>,
{
    if events.has_messages::<M>() {
        let mut message_event_writer = world
            .get_resource_mut::<Events<MessageEvent<M, Ctx>>>()
            .unwrap();
        for (message, ctx) in events.into_iter_messages::<M>() {
            let message_event = MessageEvent::new(message, ctx);
            message_event_writer.send(message_event);
        }
    }
}

pub fn push_component_insert_events<
    C: Component,
    P: Protocol,
    E: IterComponentInsertEvent<P, Ctx>,
    Ctx: EventContext,
>(
    world: &mut World,
    events: &mut E,
) where
    P::ComponentKinds: FromType<C>,
    P::ComponentKinds: FromType<C>,
{
    if events.has_component_insert::<C>() {
        let mut event_writer = world
            .get_resource_mut::<Events<ComponentInsertEvent<C, Ctx>>>()
            .unwrap();
        for (entity, ctx) in events.iter_component_insert::<C>() {
            let event = ComponentInsertEvent::new(entity, ctx);
            event_writer.send(event);
        }
    }
}

pub fn push_component_remove_events<
    C: Component,
    P: Protocol,
    E: IterComponentRemoveEvent<P, Ctx>,
    Ctx: EventContext,
>(
    world: &mut World,
    events: &mut E,
) where
    P::ComponentKinds: FromType<C>,
{
    if events.has_component_remove::<C>() {
        let mut event_writer = world
            .get_resource_mut::<Events<ComponentRemoveEvent<C, Ctx>>>()
            .unwrap();
        for (entity, ctx) in events.iter_component_remove::<C>() {
            let event = ComponentRemoveEvent::new(entity, ctx);
            event_writer.send(event);
        }
    }
}

pub fn push_component_update_events<
    C: Component,
    P: Protocol,
    E: IterComponentUpdateEvent<P, Ctx>,
    Ctx: EventContext,
>(
    world: &mut World,
    events: &mut E,
) where
    P::ComponentKinds: FromType<C>,
{
    if events.has_component_update::<C>() {
        let mut event_writer = world
            .get_resource_mut::<Events<ComponentUpdateEvent<C, Ctx>>>()
            .unwrap();
        for (entity, ctx) in events.iter_component_update::<C>() {
            let event = ComponentUpdateEvent::new(entity, ctx);
            event_writer.send(event);
        }
    }
}
