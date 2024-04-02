//! Create the bevy [`Plugin`]

use bevy::app::{App, PostUpdate};
use bevy::prelude::Plugin;

use crate::_reexport::{ComponentProtocol, EventContext, MessageProtocol, ReplicationSend};
use crate::prelude::Protocol;
use crate::shared::events::components::{
    ConnectEvent, DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent,
};
use crate::shared::sets::InternalMainSet;

pub struct EventsPlugin<P, Ctx> {
    marker: std::marker::PhantomData<(P, Ctx)>,
}

impl<P, Ctx> Default for EventsPlugin<P, Ctx> {
    fn default() -> Self {
        Self {
            marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol, Ctx: EventContext> Plugin for EventsPlugin<P, Ctx> {
    fn build(&self, app: &mut App) {
        // EVENTS
        // per-component events
        P::Components::add_events::<Ctx>(app);
        P::Message::add_events::<Ctx>(app);

        app.add_event::<ConnectEvent<Ctx>>()
            .add_event::<DisconnectEvent<Ctx>>()
            .add_event::<EntitySpawnEvent<Ctx>>()
            .add_event::<EntityDespawnEvent<Ctx>>();
    }
}
