//! Create the bevy [`Plugin`]

use bevy::app::{App, PreUpdate};
use bevy::prelude::{IntoSystemConfigs, Plugin, PostUpdate};

use crate::shared::events::components::{
    ConnectEvent, DisconnectEvent, EntityDespawnEvent, EntitySpawnEvent,
};
use crate::shared::events::systems::{clear_events, push_entity_events};
use crate::shared::replication::ReplicationReceive;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};

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
        app.add_event::<ConnectEvent<R::EventContext>>()
            .add_event::<DisconnectEvent<R::EventContext>>()
            .add_event::<EntitySpawnEvent<R::EventContext>>()
            .add_event::<EntityDespawnEvent<R::EventContext>>();
        // SYSTEMS
        app.add_systems(
            PreUpdate,
            push_entity_events::<R>.in_set(InternalMainSet::<R::SetMarker>::EmitEvents),
        );
        app.add_systems(
            PostUpdate,
            // NOTE: we add this to the All system-set so that this system doesn't run if the host is disconnected
            clear_events::<R>.in_set(InternalReplicationSet::<R::SetMarker>::All),
        );
    }
}
