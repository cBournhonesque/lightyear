//! Create the bevy [`Plugin`]

use bevy::app::App;
use bevy::prelude::Plugin;

use crate::_internal::EventContext;
use crate::shared::events::components::{
    ConnectEvent, DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent,
};

pub struct EventsPlugin<Ctx> {
    marker: std::marker::PhantomData<Ctx>,
}

impl<Ctx> Default for EventsPlugin<Ctx> {
    fn default() -> Self {
        Self {
            marker: std::marker::PhantomData,
        }
    }
}

impl<Ctx: EventContext> Plugin for EventsPlugin<Ctx> {
    fn build(&self, app: &mut App) {
        // EVENTS
        // per-component events
        // P::Components::add_events::<Ctx>(app);
        // P::Message::add_events::<Ctx>(app);

        app.add_event::<ConnectEvent<Ctx>>()
            .add_event::<DisconnectEvent<Ctx>>()
            .add_event::<EntitySpawnEvent<Ctx>>()
            .add_event::<EntityDespawnEvent<Ctx>>();
    }
}
