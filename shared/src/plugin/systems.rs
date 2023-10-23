use bevy_app::{App, PostUpdate};
use bevy_ecs::change_detection::Ref;
use bevy_ecs::prelude::{
    Added, Component, DetectChanges, Entity, EventReader, IntoSystemConfigs, ResMut,
};
use bevy_ecs::system::Query;
use std::ops::Deref;
use tracing::debug;

use crate::replication::{Replicate, ReplicationSend};
use crate::{ConnectEvent, Protocol, ReplicationSet};

pub fn send_entity_spawn<P: Protocol, R: ReplicationSend<P>>(
    // try doing entity spawn whenever replicate gets added
    query: Query<(Entity, Ref<Replicate>)>,
    mut connect_events: EventReader<ConnectEvent>,
    // query: Query<(Entity, &Replicate)>,
    mut sender: ResMut<R>,
) {
    // We might want to replicate all entities on connect
    for event in connect_events.iter() {
        let client_id = event.0;
        query.iter().for_each(|(entity, replicate)| {
            if replicate.target.should_replicate_to(client_id) {
                sender
                    .entity_spawn(entity, vec![], replicate.deref())
                    .unwrap();
            }
        })
    }

    // Replicate to already connected clients (replicate only new entities)
    query.iter().for_each(|(entity, replicate)| {
        if replicate.is_added() {
            sender
                .entity_spawn(entity, vec![], replicate.deref())
                .unwrap();
        }
    })
}

pub fn send_component_update<C: Component + Clone, P: Protocol, R: ReplicationSend<P>>(
    query: Query<(Entity, Ref<C>, &Replicate)>,
    mut connect_events: EventReader<ConnectEvent>,
    mut sender: ResMut<R>,
) where
    <P as Protocol>::Components: From<C>,
{
    // We might want to replicate the component on connect
    for event in connect_events.iter() {
        let client_id = event.0;
        query.iter().for_each(|(entity, component, replicate)| {
            if replicate.target.should_replicate_to(client_id) {
                sender
                    .component_insert(entity, component.clone().into(), replicate)
                    .unwrap();
            }
        })
    }

    query.iter().for_each(|(entity, component, replicate)| {
        // send an component_insert for components that were newly added
        if component.is_added() {
            sender
                .component_insert(entity, component.clone().into(), replicate)
                .unwrap();
        }
        // only update components that were not newly added ?
        if component.is_changed() && !component.is_added() {
            sender
                .entity_update_single_component(entity, component.clone().into(), replicate)
                .unwrap();
        }
    })
}

pub fn add_replication_send_systems<C: Component + Clone, P: Protocol, R: ReplicationSend<P>>(
    app: &mut App,
) where
    <P as Protocol>::Components: From<C>,
{
    app.add_systems(
        PostUpdate,
        (send_entity_spawn::<P, R>, send_component_update::<C, P, R>).in_set(ReplicationSet::Send),
    );
}
