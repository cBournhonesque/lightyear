use std::ops::Deref;

use bevy::prelude::{
    Added, App, Commands, Component, DetectChanges, Entity, EventReader, IntoSystemConfigs,
    PostUpdate, Query, Ref, RemovedComponents, ResMut,
};
use tracing::{debug, info};

use crate::netcode::ClientId;
use crate::protocol::component::IntoKind;
use crate::protocol::Protocol;
use crate::shared::events::ConnectEvent;
use crate::shared::replication::components::{DespawnTracker, Replicate};
use crate::shared::replication::resources::ReplicationData;
use crate::shared::replication::ReplicationSend;
use crate::shared::sets::ReplicationSet;

// TODO: make this more generic so that we can run it on both server and client
//  client might want to replicate some things to server?

// TODO: run these systems only if there is at least 1 remote connected!!!

/// This system adds DespawnTracker to each entity that was every replicated, so that we can track
/// when they are despawned
fn add_despawn_tracker(
    mut replication: ResMut<ReplicationData>,
    mut commands: Commands,
    query: Query<(Entity, &Replicate), Added<Replicate>>,
) {
    for (entity, replicate) in query.iter() {
        debug!("Adding DespawnTracker to entity: {:?}", entity);
        commands.entity(entity).insert(DespawnTracker);
        replication.owned_entities.insert(entity, *replicate);
    }
}

fn send_entity_despawn<P: Protocol, R: ReplicationSend<P>>(
    mut replication: ResMut<ReplicationData>,
    // query: Query<(Entity, Ref<Replicate>), RemovedComponents<>>
    // TODO: ideally we want to send despawns for entities that still had REPLICATE at the time of despawn
    mut despawn_removed: RemovedComponents<DespawnTracker>,
    mut sender: ResMut<R>,
) {
    for entity in despawn_removed.read() {
        if let Some(replicate) = replication.owned_entities.remove(&entity) {
            sender.entity_despawn(entity, &replicate).unwrap();
        }
    }
}

// TODO: maybe there was no point in making this generic in replication send; because
//  connect-events is only available on the server ? or should we also add it in the client ?
//  we can also separate the on_connect part to a separate system
fn send_entity_spawn<P: Protocol, R: ReplicationSend<P>>(
    mut replication: ResMut<ReplicationData>,
    // try doing entity spawn whenever replicate gets added
    query: Query<(Entity, Ref<Replicate>)>,
    // TODO: use Ctx instead of ClientId so that this can be used on both client and server
    mut connect_events: EventReader<ConnectEvent<ClientId>>,
    // query: Query<(Entity, &Replicate)>,
    mut sender: ResMut<R>,
) {
    // We might want to replicate all entities on connect
    for event in connect_events.read() {
        let client_id = event.context();
        query.iter().for_each(|(entity, replicate)| {
            if replicate.replication_target.should_send_to(client_id) {
                sender
                    .entity_spawn(entity, vec![], replicate.deref())
                    .unwrap();
            }
        })
    }

    // Replicate to already connected clients (replicate only new entities)
    query.iter().for_each(|(entity, replicate)| {
        if replicate.is_added() {
            replication.owned_entities.insert(entity, *replicate);
            sender
                .entity_spawn(entity, vec![], replicate.deref())
                .unwrap();
        }
    })
}

/// This system sends updates for all components that were added or changed
/// Sends both ComponentInsert for newly added components
/// and ComponentUpdates otherwise
fn send_component_update<C: Component + Clone, P: Protocol, R: ReplicationSend<P>>(
    query: Query<(Entity, Ref<C>, &Replicate)>,
    // TODO: use Ctx instead of ClientId so that this can be used on both client and server
    mut connect_events: EventReader<ConnectEvent<ClientId>>,
    mut sender: ResMut<R>,
) where
    <P as Protocol>::Components: From<C>,
{
    // We might want to replicate the component on connect
    for event in connect_events.read() {
        let client_id = event.context();
        query.iter().for_each(|(entity, component, replicate)| {
            if replicate.replication_target.should_send_to(client_id) {
                sender
                    .component_insert(entity, component.clone().into(), replicate)
                    .unwrap();
            }
        })
    }

    // TODO: find a way to not do this if we already sent messages in the previous loops for newly connected clients
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

/// This system sends updates for all components that were removed
fn send_component_removed<C: Component + Clone, P: Protocol, R: ReplicationSend<P>>(
    // only remove the component for entities that are being actively replicated
    query: Query<&Replicate>,
    mut removed: RemovedComponents<C>,
    mut sender: ResMut<R>,
) where
    C: IntoKind<<P as Protocol>::ComponentKinds>,
{
    removed.read().for_each(|entity| {
        if let Ok(replicate) = query.get(entity) {
            sender
                .component_remove(entity, C::into_kind(), replicate)
                .unwrap()
        }
    })
}

pub fn add_replication_send_systems<P: Protocol, R: ReplicationSend<P>>(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (
            add_despawn_tracker,
            send_entity_spawn::<P, R>,
            send_entity_despawn::<P, R>,
        )
            .in_set(ReplicationSet::SendEntityUpdates),
    );
}

// pub fn add_replication_send_systems
pub fn add_per_component_replication_send_systems<
    C: Component + Clone,
    P: Protocol,
    R: ReplicationSend<P>,
>(
    app: &mut App,
) where
    <P as Protocol>::Components: From<C>,
    C: IntoKind<<P as Protocol>::ComponentKinds>,
{
    app.add_systems(
        PostUpdate,
        (
            send_component_removed::<C, P, R>,
            send_component_update::<C, P, R>,
        )
            .in_set(ReplicationSet::SendComponentUpdates),
    );
}
