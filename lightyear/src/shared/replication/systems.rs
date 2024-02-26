//! Bevy [`bevy::prelude::System`]s used for replication
use std::ops::Deref;

use bevy::ecs::entity::Entities;
use bevy::ecs::system::SystemChangeTick;
use bevy::prelude::{
    Added, App, Commands, Component, DetectChanges, Entity, IntoSystemConfigs, PostUpdate,
    PreUpdate, Query, Ref, RemovedComponents, Res, ResMut, Without,
};
use tracing::{debug, error, info, trace};

use crate::_reexport::FromType;
use crate::prelude::{MainSet, NetworkTarget, TickManager};
use crate::protocol::Protocol;
use crate::server::room::ClientVisibility;
use crate::shared::replication::components::{DespawnTracker, Replicate, ReplicationMode};
use crate::shared::replication::ReplicationSend;
use crate::shared::sets::ReplicationSet;

// TODO: run these systems only if there is at least 1 remote connected!!! (so we don't burn CPU when there are no connections)

/// For every entity that removes their Replicate component but are not despawned, remove the component
/// from our replicate cache (so that the entity's despawns are no longer replicated)
fn handle_replicate_remove<P: Protocol, R: ReplicationSend<P>>(
    mut sender: ResMut<R>,
    mut query: RemovedComponents<Replicate<P>>,
    entity_check: &Entities,
) {
    for entity in query.read() {
        if entity_check.contains(entity) {
            debug!("handling replicate component remove (delete from cache)");
            sender.get_mut_replicate_component_cache().remove(&entity);
        }
    }
}

// TODO: maybe only store in the replicate_component_cache the things we need for despawn, which are just replication-target and group-id?
//  the rest is a waste of memory
/// This system adds DespawnTracker to each entity that was every replicated,
/// so that we can track when they are despawned
/// (we have a distinction between removing Replicate, which just stops replication; and despawning the entity)
fn add_despawn_tracker<P: Protocol, R: ReplicationSend<P>>(
    mut sender: ResMut<R>,
    mut commands: Commands,
    query: Query<(Entity, &Replicate<P>), (Added<Replicate<P>>, Without<DespawnTracker>)>,
) {
    for (entity, replicate) in query.iter() {
        debug!("ADDING DESPAWN TRACKER");
        commands.entity(entity).insert(DespawnTracker);
        sender
            .get_mut_replicate_component_cache()
            .insert(entity, replicate.clone());
    }
}

fn send_entity_despawn<P: Protocol, R: ReplicationSend<P>>(
    query: Query<(Entity, &Replicate<P>)>,
    system_bevy_ticks: SystemChangeTick,
    // TODO: ideally we want to send despawns for entities that still had REPLICATE at the time of despawn
    //  not just entities that had despawn tracker once
    mut despawn_removed: RemovedComponents<DespawnTracker>,
    mut sender: ResMut<R>,
) {
    // Despawn entities for clients that lost visibility
    query.iter().for_each(|(entity, replicate)| {
        if matches!(replicate.replication_mode, ReplicationMode::Room) {
            replicate
                .replication_clients_cache
                .iter()
                .for_each(|(client_id, visibility)| {
                    if replicate.replication_target.should_send_to(client_id)
                        && matches!(visibility, ClientVisibility::Lost)
                    {
                        debug!("sending entity despawn for entity: {:?}", entity);
                        // TODO: don't unwrap but handle errors
                        let _ = sender
                            .prepare_entity_despawn(
                                entity,
                                replicate,
                                NetworkTarget::Only(vec![*client_id]),
                                system_bevy_ticks.this_run(),
                            )
                            .map_err(|e| {
                                error!("error sending entity despawn: {:?}", e);
                            });
                    }
                });
        }
    });

    // Despawn entities when the entity got despawned on local world
    for entity in despawn_removed.read() {
        trace!("despawn tracker removed!");
        // only replicate the despawn if the entity still had a Replicate component
        if let Some(replicate) = sender.get_mut_replicate_component_cache().remove(&entity) {
            // TODO: DO NOT SEND ENTITY DESPAWN TO THE CLIENT WHO JUST DISCONNECTED!
            trace!("send entity despawn");
            let _ = sender
                .prepare_entity_despawn(
                    entity,
                    &replicate,
                    replicate.replication_target.clone(),
                    system_bevy_ticks.this_run(),
                )
                // TODO: bubble up errors to user via ConnectionEvents
                //  use thiserror so that user can distinguish between error types
                .map_err(|e| {
                    error!("error sending entity despawn: {:?}", e);
                });
        }
    }
}

fn send_entity_spawn<P: Protocol, R: ReplicationSend<P>>(
    system_bevy_ticks: SystemChangeTick,
    query: Query<(Entity, Ref<Replicate<P>>)>,
    mut sender: ResMut<R>,
) {
    // Replicate to already connected clients (replicate only new entities)
    query.iter().for_each(|(entity, replicate)| {
        match replicate.replication_mode {
            // for room mode, no need to handle newly-connected clients specially; they just need
            // to be added to the correct room
            ReplicationMode::Room => {
                replicate
                    .replication_clients_cache
                    .iter()
                    .for_each(|(client_id, visibility)| {
                        if replicate.replication_target.should_send_to(client_id) {
                            match visibility {
                                ClientVisibility::Gained => {
                                    debug!("send entity spawn to gained");
                                    let _ = sender
                                        .prepare_entity_spawn(
                                            entity,
                                            &replicate,
                                            NetworkTarget::Only(vec![*client_id]),
                                            system_bevy_ticks.this_run(),
                                        )
                                        .map_err(|e| {
                                            error!("error sending entity spawn: {:?}", e);
                                        });
                                }
                                ClientVisibility::Lost => {}
                                ClientVisibility::Maintained => {
                                    // TODO: is this even reachable?
                                    // only try to replicate if the replicate component was just added
                                    if replicate.is_added() {
                                        debug!("send entity spawn to maintained");
                                        sender
                                            .get_mut_replicate_component_cache()
                                            .insert(entity, replicate.clone());
                                        let _ = sender
                                            .prepare_entity_spawn(
                                                entity,
                                                replicate.deref(),
                                                NetworkTarget::Only(vec![*client_id]),
                                                system_bevy_ticks.this_run(),
                                            )
                                            .map_err(|e| {
                                                error!("error sending entity spawn: {:?}", e);
                                            });
                                    }
                                }
                            }
                        }
                    });
            }
            ReplicationMode::NetworkTarget => {
                let mut target = replicate.replication_target.clone();

                let new_connected_clients = sender.new_connected_clients().clone();
                if !new_connected_clients.is_empty() {
                    // replicate to the newly connected clients that match our target
                    let mut new_connected_target = target.clone();
                    new_connected_target
                        .intersection(NetworkTarget::Only(new_connected_clients.clone()));
                    // replicate all entities to newly connected clients
                    let _ = sender
                        .prepare_entity_spawn(
                            entity,
                            &replicate,
                            new_connected_target,
                            system_bevy_ticks.this_run(),
                        )
                        .map_err(|e| {
                            error!("error sending entity spawn: {:?}", e);
                        });
                    // don't re-send to newly connection client
                    target.exclude(new_connected_clients.clone());
                }

                // only try to replicate if the replicate component was just added
                if replicate.is_added() {
                    trace!(?entity, "send entity spawn");
                    sender
                        .get_mut_replicate_component_cache()
                        .insert(entity, replicate.clone());
                    let _ = sender
                        .prepare_entity_spawn(
                            entity,
                            replicate.deref(),
                            target,
                            system_bevy_ticks.this_run(),
                        )
                        .map_err(|e| {
                            error!("error sending entity spawn: {:?}", e);
                        });
                }
            }
        }
    })
}

/// This system sends updates for all components that were added or changed
/// Sends both ComponentInsert for newly added components
/// and ComponentUpdates otherwise
///
/// Updates are sent only for any components that were changed since the most recent of:
/// - last time we sent an action for that group
/// - last time we sent an update for that group which got acked.
/// (currently we only check for the second condition, which is enough but less efficient)
///
/// NOTE: cannot use ConnectEvents because they are reset every frame
fn send_component_update<C: Component + Clone, P: Protocol, R: ReplicationSend<P>>(
    query: Query<(Entity, Ref<C>, &Replicate<P>)>,
    system_bevy_ticks: SystemChangeTick,
    mut sender: ResMut<R>,
) where
    <P as Protocol>::Components: From<C>,
    P::ComponentKinds: FromType<C>,
{
    let kind = <P::ComponentKinds as FromType<C>>::from_type();
    query.iter().for_each(|(entity, component, replicate)| {
        // do not replicate components that are disabled
        if replicate.is_disabled::<C>() {
            return;
        }
        match replicate.replication_mode {
            ReplicationMode::Room => {
                replicate
                    .replication_clients_cache
                    .iter()
                    .for_each(|(client_id, visibility)| {
                        if replicate.replication_target.should_send_to(client_id) {
                            match visibility {
                                ClientVisibility::Gained => {
                                    let target = replicate.target::<C>(NetworkTarget::Only(vec![*client_id]));
                                    let _ = sender
                                        .prepare_component_insert(
                                            entity,
                                            component.clone().into(),
                                            replicate,
                                            target,
                                            system_bevy_ticks.this_run(),
                                        )
                                        .map_err(|e| {
                                            error!("error sending component insert: {:?}", e);
                                        });
                                }
                                ClientVisibility::Lost => {}
                                ClientVisibility::Maintained => {
                                    // send an component_insert for components that were newly added
                                    if component.is_added() {
                                        let target = replicate.target::<C>(NetworkTarget::Only(vec![*client_id]));
                                        let _ = sender
                                            .prepare_component_insert(
                                                entity,
                                                component.clone().into(),
                                                replicate,
                                                target,
                                                system_bevy_ticks.this_run(),
                                            )
                                            .map_err(|e| {
                                                error!("error sending component insert: {:?}", e);
                                            });
                                        // only update components that were not newly added
                                    } else {
                                        // do not send updates for these components, only inserts/removes
                                        if replicate.is_replicate_once::<C>() {
                                            return;
                                        }
                                        let target = replicate.target::<C>(NetworkTarget::Only(vec![*client_id]));
                                        let _ = sender
                                            .prepare_entity_update(
                                                entity,
                                                component.clone().into(),
                                                replicate,
                                                target,
                                                component.last_changed(),
                                                system_bevy_ticks.this_run(),
                                            )
                                            .map_err(|e| {
                                                error!("error sending component update: {:?}", e);
                                            });
                                    }
                                }
                            }
                        }
                    })
            }
            ReplicationMode::NetworkTarget => {
                let mut target = replicate.replication_target.clone();

                let new_connected_clients = sender.new_connected_clients().clone();
                // replicate all components to newly connected clients
                if !new_connected_clients.is_empty() {
                    // replicate to the newly connected clients that match our target
                    let mut new_connected_target = target.clone();
                    new_connected_target
                        .intersection(NetworkTarget::Only(new_connected_clients.clone()));
                    let _ = sender
                        .prepare_component_insert(
                            entity,
                            component.clone().into(),
                            replicate,
                            replicate.target::<C>(new_connected_target),
                            system_bevy_ticks.this_run(),
                        )
                        .map_err(|e| {
                            error!("error sending component insert: {:?}", e);
                        });
                    // don't re-send to newly connection client
                    target.exclude(new_connected_clients.clone());
                }

                // send an component_insert for components that were newly added
                if component.is_added() {
                    trace!("component is added");
                    let _ = sender
                        .prepare_component_insert(
                            entity,
                            component.clone().into(),
                            replicate,
                            replicate.target::<C>(target),
                            system_bevy_ticks.this_run(),
                        )
                        .map_err(|e| {
                            error!("error sending component insert: {:?}", e);
                        });
                } else {
                    // do not send updates for these components, only inserts/removes
                    if replicate.is_replicate_once::<C>() {
                        trace!(?entity,
                            "not replicating updates for {:?} because it is marked as replicate_once",
                            kind
                        );
                        return;
                    }
                    // otherwise send an update for all components that changed since the
                    // last update we have ack-ed
                    let _ = sender
                        .prepare_entity_update(
                            entity,
                            component.clone().into(),
                            replicate,
                            replicate.target::<C>(target),
                            component.last_changed(),
                            system_bevy_ticks.this_run(),
                        )
                        .map_err(|e| {
                            error!("error sending component update: {:?}", e);
                        });
                }
            }
        }
    });
}

/// This system sends updates for all components that were removed
fn send_component_removed<C: Component + Clone, P: Protocol, R: ReplicationSend<P>>(
    // only remove the component for entities that are being actively replicated
    query: Query<&Replicate<P>>,
    system_bevy_ticks: SystemChangeTick,
    mut removed: RemovedComponents<C>,
    mut sender: ResMut<R>,
) where
    P::ComponentKinds: FromType<C>,
{
    let kind = <P::ComponentKinds as FromType<C>>::from_type();
    removed.read().for_each(|entity| {
        if let Ok(replicate) = query.get(entity) {
            // do not replicate components that are disabled
            if replicate.is_disabled::<C>() {
                return;
            }
            match replicate.replication_mode {
                ReplicationMode::Room => {
                    replicate.replication_clients_cache.iter().for_each(
                        |(client_id, visibility)| {
                            if replicate.replication_target.should_send_to(client_id) {
                                // TODO: maybe send no matter the vis?
                                if matches!(visibility, ClientVisibility::Maintained) {
                                    let _ = sender
                                        .prepare_component_remove(
                                            entity,
                                            kind,
                                            replicate,
                                            replicate
                                                .target::<C>(NetworkTarget::Only(vec![*client_id])),
                                            system_bevy_ticks.this_run(),
                                        )
                                        .map_err(|e| {
                                            error!("error sending component remove: {:?}", e);
                                        });
                                }
                            }
                        },
                    )
                }
                ReplicationMode::NetworkTarget => {
                    trace!("sending component remove!");
                    let _ = sender
                        .prepare_component_remove(
                            entity,
                            kind,
                            replicate,
                            replicate.target::<C>(replicate.replication_target.clone()),
                            system_bevy_ticks.this_run(),
                        )
                        .map_err(|e| {
                            error!("error sending component remove: {:?}", e);
                        });
                }
            }
        }
    })
}

// add replication systems that are shared between client and server
pub fn add_replication_send_systems<P: Protocol, R: ReplicationSend<P>>(app: &mut App) {
    // we need to add despawn trackers immediately for entities for which we add replicate
    app.add_systems(
        PreUpdate,
        add_despawn_tracker::<P, R>.after(MainSet::ClientReplicationFlush),
    );
    app.add_systems(
        PostUpdate,
        (
            // TODO: try to move this to ReplicationSystems as well? entities are spawned only once
            //  so we can run the system every frame
            //  putting it here means we might miss entities that are spawned and depspawned within the send_interval? bug or feature?
            send_entity_spawn::<P, R>.in_set(ReplicationSet::SendEntityUpdates),
            // NOTE: we need to run `send_entity_despawn` once per frame (and not once per send_interval)
            //  because the RemovedComponents Events are present only for 1 frame and we might miss them if we don't run this every frame
            //  It is ok to run it every frame because it creates at most one message per despawn
            // NOTE: we make sure to update the replicate_cache before we make use of it in `send_entity_despawn`
            (
                (add_despawn_tracker::<P, R>, handle_replicate_remove::<P, R>),
                send_entity_despawn::<P, R>,
            )
                .chain()
                .in_set(ReplicationSet::SendDespawnsAndRemovals),
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
    P::Components: From<C>,
    P::ComponentKinds: FromType<C>,
{
    app.add_systems(
        PostUpdate,
        (
            // NOTE: we need to run `send_component_removed` once per frame (and not once per send_interval)
            //  because the RemovedComponents Events are present only for 1 frame and we might miss them if we don't run this every frame
            //  It is ok to run it every frame because it creates at most one message per despawn
            send_component_removed::<C, P, R>.in_set(ReplicationSet::SendDespawnsAndRemovals),
            // NOTE: we run this system once every `send_interval` because we don't want to send too many Update messages
            //  and use up all the bandwidth
            send_component_update::<C, P, R>.in_set(ReplicationSet::SendComponentUpdates),
        ),
    );
}

pub(crate) fn cleanup<P: Protocol, R: ReplicationSend<P>>(
    mut sender: ResMut<R>,
    tick_manager: Res<TickManager>,
) {
    let tick = tick_manager.tick();
    sender.cleanup(tick);
}
