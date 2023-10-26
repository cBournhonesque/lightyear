use bevy::prelude::{
    Added, App, Component, DetectChanges, Entity, EventReader, IntoSystemConfigs, PostUpdate,
    Query, Ref, ResMut,
};
use std::ops::Deref;
use tracing::debug;

use crate::connection::events::EventContext;
use crate::replication::{Replicate, ReplicationSend};
use crate::ClientId;
use crate::{ConnectEvent, Protocol, ReplicationSet};

// TODO: make this more generic so that we can run it on both server and client
//  client might want to replicate some things to server?

// TODO: run these systems only if there is at least 1 remote connected!!!

// TODO: maybe there was no point in making this generic in replication send; because
//  connect-events is only available on the server ? or should we also add it in the client ?
//  we can also separate the on_connect part to a separate system
fn send_entity_spawn<P: Protocol, R: ReplicationSend<P>>(
    // try doing entity spawn whenever replicate gets added
    query: Query<(Entity, Ref<Replicate>)>,
    mut connect_events: EventReader<ConnectEvent<ClientId>>,
    // query: Query<(Entity, &Replicate)>,
    mut sender: ResMut<R>,
) {
    // We might want to replicate all entities on connect
    for event in connect_events.iter() {
        let client_id = event.context();
        query.iter().for_each(|(entity, replicate)| {
            if replicate.target.should_replicate_to(client_id.clone()) {
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

fn send_component_update<C: Component + Clone, P: Protocol, R: ReplicationSend<P>>(
    query: Query<(Entity, Ref<C>, &Replicate)>,
    mut connect_events: EventReader<ConnectEvent<ClientId>>,
    mut sender: ResMut<R>,
) where
    <P as Protocol>::Components: From<C>,
{
    // We might want to replicate the component on connect
    for event in connect_events.iter() {
        let client_id = event.context();
        query.iter().for_each(|(entity, component, replicate)| {
            if replicate.target.should_replicate_to(client_id.clone()) {
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
