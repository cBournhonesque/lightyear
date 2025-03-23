//! Create the bevy [`Plugin`]

use bevy::app::{App, PreUpdate};
use bevy::prelude::*;

use crate::shared::events::components::{EntityDespawnEvent, EntitySpawnEvent};
use crate::shared::events::systems::{clear_events, push_entity_events};
use crate::shared::replication::ReplicationReceive;
use crate::shared::sets::InternalMainSet;

pub struct EventsPlugin<R> {
    marker: core::marker::PhantomData<R>,
}

impl<R> Default for EventsPlugin<R> {
    fn default() -> Self {
        Self {
            marker: core::marker::PhantomData,
        }
    }
}

impl<R: ReplicationReceive> Plugin for EventsPlugin<R> {
    fn build(&self, app: &mut App) {
        // EVENTS
        app.add_event::<EntitySpawnEvent<R::EventContext>>()
            .add_event::<EntityDespawnEvent<R::EventContext>>();
        // SYSTEMS
        app.add_systems(
            PreUpdate,
            push_entity_events::<R>.in_set(InternalMainSet::<R::SetMarker>::ReceiveEvents),
        );
        app.add_systems(
            PreUpdate,
            clear_events::<R>.after(InternalMainSet::<R::SetMarker>::ReceiveEvents),
        );
    }
}
