//! Bevy [`bevy::prelude::System`]s used for replication
use std::ops::Deref;

use bevy::prelude::{
    Added, App, Commands, Component, DetectChanges, Entity, EventReader, IntoSystemConfigs, Mut,
    PostUpdate, Query, Ref, RemovedComponents, ResMut,
};
use tracing::{debug, info};

use crate::netcode::ClientId;
use crate::prelude::NetworkTarget;
use crate::protocol::component::IntoKind;
use crate::protocol::Protocol;
use crate::server::room::ClientVisibility;
use crate::shared::events::ConnectEvent;
use crate::shared::replication::components::{DespawnTracker, Replicate};
use crate::shared::replication::resources::ReplicationData;
use crate::shared::replication::ReplicationSend;
use crate::shared::sets::ReplicationSet;

// TODO: make this more generic so that we can run it on both server and client
//  client might want to replicate some things to server?

// TODO: run these systems only if there is at least 1 remote connected!!!

/// This system adds DespawnTracker to each entity that was every replicated,
/// so that we can track when they are despawned
fn add_despawn_tracker(
    mut replication: ResMut<ReplicationData>,
    mut commands: Commands,
    query: Query<(Entity, &Replicate), Added<Replicate>>,
) {
    for (entity, replicate) in query.iter() {
        commands.entity(entity).insert(DespawnTracker);
        replication.owned_entities.insert(entity, replicate.clone());
    }
}

fn send_entity_despawn<P: Protocol, R: ReplicationSend<P>>(
    mut replication: ResMut<ReplicationData>,
    query: Query<(Entity, Ref<Replicate>)>,
    // TODO: ideally we want to send despawns for entities that still had REPLICATE at the time of despawn
    //  not just entities that had despawn tracker once
    mut despawn_removed: RemovedComponents<DespawnTracker>,
    mut sender: ResMut<R>,
) {
    // Despawn entities for clients that lost visibility
    query.iter().for_each(|(entity, replicate)| {
        replicate
            .replication_clients_cache
            .iter()
            .for_each(|(client_id, visibility)| {
                if replicate.replication_target.should_send_to(client_id) {
                    if matches!(visibility, ClientVisibility::Lost) {
                        sender
                            .entity_despawn(entity, &replicate, NetworkTarget::Only(*client_id))
                            .unwrap();
                    }
                }
            });
    });
    // Despawn entities when the entity got despawned on local world
    for entity in despawn_removed.read() {
        if let Some(replicate) = replication.owned_entities.remove(&entity) {
            // TODO: maybe check the status of replicate.replication_clients_cache
            //  and only despawn for the entities in the cache?
            //  but that means we have to update the owned_entity value every time the replication_clients_cache is updated
            sender
                .entity_despawn(entity, &replicate, replicate.replication_target)
                .unwrap();
        }
    }
}

// TODO: maybe there was no point in making this generic in replication send; because
//  connect-events is only available on the server ? or should we also add it in the client ?
//  we can also separate the on_connect part to a separate system
fn send_entity_spawn<P: Protocol, R: ReplicationSend<P>>(
    mut replication: ResMut<ReplicationData>,
    query: Query<(Entity, Ref<Replicate>)>,
    mut sender: ResMut<R>,
) {
    // Replicate to already connected clients (replicate only new entities)
    query.iter().for_each(|(entity, replicate)| {
        replicate
            .replication_clients_cache
            .iter()
            .for_each(|(client_id, visibility)| {
                if replicate.replication_target.should_send_to(client_id) {
                    match visibility {
                        ClientVisibility::Gained => {
                            sender
                                .entity_spawn(
                                    entity,
                                    vec![],
                                    &replicate,
                                    NetworkTarget::Only(*client_id),
                                )
                                .unwrap();
                        }
                        ClientVisibility::Lost => {}
                        ClientVisibility::Maintained => {
                            // only try to replicate if the replicate component was just added
                            if replicate.is_added() {
                                replication.owned_entities.insert(entity, replicate.clone());
                                sender
                                    .entity_spawn(
                                        entity,
                                        vec![],
                                        replicate.deref(),
                                        NetworkTarget::Only(*client_id),
                                    )
                                    .unwrap();
                            }
                        }
                    }
                }
            });
    })
}

/// This system sends updates for all components that were added or changed
/// Sends both ComponentInsert for newly added components
/// and ComponentUpdates otherwise
fn send_component_update<C: Component + Clone, P: Protocol, R: ReplicationSend<P>>(
    query: Query<(Entity, Ref<C>, &Replicate)>,
    mut sender: ResMut<R>,
) where
    <P as Protocol>::Components: From<C>,
{
    query.iter().for_each(|(entity, component, replicate)| {
        replicate
            .replication_clients_cache
            .iter()
            .for_each(|(client_id, visibility)| {
                if replicate.replication_target.should_send_to(&client_id) {
                    match visibility {
                        ClientVisibility::Gained => {
                            sender
                                .component_insert(
                                    entity,
                                    component.clone().into(),
                                    &replicate,
                                    NetworkTarget::Only(*client_id),
                                )
                                .unwrap();
                        }
                        ClientVisibility::Lost => {}
                        ClientVisibility::Maintained => {
                            // send an component_insert for components that were newly added
                            if component.is_added() {
                                sender
                                    .component_insert(
                                        entity,
                                        component.clone().into(),
                                        &replicate,
                                        NetworkTarget::Only(*client_id),
                                    )
                                    .unwrap();
                            // only update components that were not newly added
                            } else if component.is_changed() {
                                sender
                                    .entity_update_single_component(
                                        entity,
                                        component.clone().into(),
                                        &replicate,
                                        NetworkTarget::Only(*client_id),
                                    )
                                    .unwrap();
                            }
                        }
                    }
                }
            })
    });
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
            replicate
                .replication_clients_cache
                .iter()
                .for_each(|(client_id, visibility)| {
                    if replicate.replication_target.should_send_to(&client_id) {
                        // TODO: maybe send no matter the vis?
                        if matches!(visibility, ClientVisibility::Maintained) {
                            sender
                                .component_remove(
                                    entity,
                                    C::into_kind(),
                                    &replicate,
                                    NetworkTarget::Only(*client_id),
                                )
                                .unwrap();
                        }
                    }
                })
        }
    })
}

pub fn add_replication_send_systems<P: Protocol, R: ReplicationSend<P>>(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (
            // TODO: try to move this to ReplicationSystems as well? entities are spawned only once
            //  so we can run the system every frame
            send_entity_spawn::<P, R>.in_set(ReplicationSet::SendEntityUpdates),
            // NOTE: we need to run `send_entity_despawn` once per frame (and not once per send_interval)
            //  because the RemovedComponents Events are present only for 1 frame and we might miss them if we don't run this every frame
            //  It is ok to run it every frame because it creates at most one message per despawn
            (add_despawn_tracker, send_entity_despawn::<P, R>)
                .in_set(ReplicationSet::ReplicationSystems),
        ),
    );
}

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
            // NOTE: we need to run `send_component_removed` once per frame (and not once per send_interval)
            //  because the RemovedComponents Events are present only for 1 frame and we might miss them if we don't run this every frame
            //  It is ok to run it every frame because it creates at most one message per despawn
            send_component_removed::<C, P, R>.in_set(ReplicationSet::ReplicationSystems),
            // NOTE: we run this system once every `send_interval` because we don't want to send too many Update messages
            //  and use up all the bandwidth
            send_component_update::<C, P, R>.in_set(ReplicationSet::SendComponentUpdates),
        ),
    );
}
