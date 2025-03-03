//! Create the bevy [`Plugin`]

use bevy::{
    app::{App, PreUpdate},
    prelude::{IntoSystemConfigs, Plugin},
};

use crate::shared::{
    events::{
        components::{EntityDespawnEvent, EntitySpawnEvent},
        systems::{clear_events, push_entity_events},
    },
    replication::ReplicationReceive,
    sets::InternalMainSet,
};

pub struct EventsPlugin<R> {
    marker: std::marker::PhantomData<R>,
}

impl<R> Default for EventsPlugin<R> {
    fn default() -> Self {
        Self {
            marker: std::marker::PhantomData,
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
