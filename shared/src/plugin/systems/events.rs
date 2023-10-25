use crate::connection::events::{EventContext, IterMessageEvent};
use crate::plugin::events::MessageEvent;
use crate::{Message, Protocol};
use bevy::prelude::{Event, Events, World};

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
