//! Bevy [`bevy::prelude::System`]s used for replication
use std::any::TypeId;
use std::ops::Deref;

use bevy::ecs::entity::Entities;
use bevy::ecs::system::SystemChangeTick;
use bevy::prelude::{
    Added, App, Commands, Component, DetectChanges, Entity, IntoSystemConfigs, Mut, PostUpdate,
    PreUpdate, Query, Ref, RemovedComponents, Res, ResMut, With, Without,
};
use tracing::{debug, error, info, trace, warn};

use crate::client::replication::ClientReplicationPlugin;
use crate::prelude::{ClientId, NetworkTarget, ReplicationGroup, ShouldBePredicted, TickManager};
use crate::protocol::component::ComponentRegistry;
use crate::server::replication::ServerReplicationSet;
use crate::server::room::ClientVisibility;
use crate::shared::replication::components::{
    DespawnTracker, Replicate, ReplicateVisibility, ReplicationGroupId, ReplicationMode,
};
use crate::shared::replication::ReplicationSend;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};

// TODO: replace this with observers
/// Metadata that holds Replicate-information (so that when the entity is despawned we know
/// how to replicate the despawn)
pub(crate) struct DespawnMetadata {
    replication_target: NetworkTarget,
    replication_group: ReplicationGroup,
    replication_mode: ReplicationMode,
    /// If mode = Room, the list of clients that could see the entity
    pub(crate) replication_clients_cache: Vec<ClientId>,
}

/// For every entity that removes their Replicate component but are not despawned, remove the component
/// from our replicate cache (so that the entity's despawns are no longer replicated)
fn handle_replicate_remove<R: ReplicationSend>(
    mut commands: Commands,
    mut sender: ResMut<R>,
    mut query: RemovedComponents<Replicate>,
    entity_check: &Entities,
) {
    for entity in query.read() {
        if entity_check.contains(entity) {
            debug!("handling replicate component remove (delete from cache)");
            sender.get_mut_replicate_despawn_cache().remove(&entity);
            commands.entity(entity).remove::<ReplicateVisibility>();
        }
    }
}

/// This system does all the additional bookkeeping required after Replicate has been added:
/// - adds DespawnTracker to each entity that was ever replicated, so that we can track when they are despawned
/// (we have a distinction between removing Replicate, which just stops replication; and despawning the entity)
/// - adds DespawnMetadata for that entity so that when it's removed, we can know how to replicate the despawn
/// - adds the ReplicateVisibility component if needed
pub(crate) fn handle_replicate_add<R: ReplicationSend>(
    mut sender: ResMut<R>,
    mut commands: Commands,
    query: Query<(Entity, &Replicate), (Added<Replicate>, Without<DespawnTracker>)>,
) {
    for (entity, replicate) in query.iter() {
        debug!("Replicate component was added");
        commands.entity(entity).insert(DespawnTracker);
        let despawn_metadata = DespawnMetadata {
            replication_target: replicate.replication_target.clone(),
            replication_group: replicate.replication_group,
            replication_mode: replicate.replication_mode,
            replication_clients_cache: vec![],
        };
        sender
            .get_mut_replicate_despawn_cache()
            .insert(entity, despawn_metadata);
        if replicate.replication_mode == ReplicationMode::Room {
            commands
                .entity(entity)
                .insert(ReplicateVisibility::default());
        }
    }
}

fn send_entity_despawn<R: ReplicationSend>(
    query: Query<(Entity, &Replicate, Option<&ReplicateVisibility>)>,
    system_bevy_ticks: SystemChangeTick,
    // TODO: ideally we want to send despawns for entities that still had REPLICATE at the time of despawn
    //  not just entities that had despawn tracker once
    mut despawn_removed: RemovedComponents<DespawnTracker>,
    mut sender: ResMut<R>,
) {
    // Despawn entities for clients that lost visibility
    query.iter().for_each(|(entity, replicate, visibility)| {
        if matches!(replicate.replication_mode, ReplicationMode::Room) {
            visibility
                .unwrap()
                .clients_cache
                .iter()
                .for_each(|(client_id, visibility)| {
                    if replicate.replication_target.should_send_to(client_id)
                        && matches!(visibility, ClientVisibility::Lost)
                    {
                        debug!("sending entity despawn for entity: {:?}", entity);
                        // TODO: don't unwrap but handle errors
                        let group_id = replicate.replication_group.group_id(Some(entity));
                        let _ = sender
                            .prepare_entity_despawn(
                                entity,
                                group_id,
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

    // TODO: check for banned replicate component?
    // Despawn entities when the entity got despawned on local world
    for entity in despawn_removed.read() {
        trace!("despawn tracker removed!");
        // TODO: we still don't want to replicate the despawn if the entity was not in the same room as the client!
        // only replicate the despawn if the entity still had a Replicate component
        if let Some(despawn_metadata) = sender.get_mut_replicate_despawn_cache().remove(&entity) {
            // TODO: DO NOT SEND ENTITY DESPAWN TO THE CLIENT WHO JUST DISCONNECTED!
            let mut network_target = despawn_metadata.replication_target;

            // TODO: for this to work properly, we need the replicate stored in `sender.get_mut_replicate_component_cache()`
            //  to be updated for every replication change! Wait for observers instead.
            //  How did it work on the `main` branch? was there something else making it work? Maybe the
            //  update replicate ran before
            if despawn_metadata.replication_mode == ReplicationMode::Room {
                // if the mode was room, only replicate the despawn to clients that were in the same room
                network_target.intersection(NetworkTarget::Only(
                    despawn_metadata.replication_clients_cache,
                ));
            }
            trace!(?entity, ?network_target, "send entity despawn");
            let group_id = despawn_metadata.replication_group.group_id(Some(entity));
            let _ = sender
                .prepare_entity_despawn(
                    entity,
                    group_id,
                    network_target,
                    system_bevy_ticks.this_run(),
                )
                // TODO: bubble up errors to user via ConnectionEvents
                .map_err(|e| {
                    error!("error sending entity despawn: {:?}", e);
                });
        }
    }
}

fn send_entity_spawn<R: ReplicationSend>(
    system_bevy_ticks: SystemChangeTick,
    component_registry: Res<ComponentRegistry>,
    query: Query<(Entity, Ref<Replicate>, Option<&ReplicateVisibility>)>,
    mut sender: ResMut<R>,
) {
    // Replicate to already connected clients (replicate only new entities)
    query.iter().for_each(|(entity, replicate, visibility)| {
        match replicate.replication_mode {
            // for room mode, no need to handle newly-connected clients specially; they just need
            // to be added to the correct room
            ReplicationMode::Room => {
                visibility.unwrap().clients_cache
                    .iter()
                    .for_each(|(client_id, visibility)| {
                        if replicate.replication_target.should_send_to(client_id) {
                            match visibility {
                                ClientVisibility::Gained => {
                                    trace!(
                                        ?entity,
                                        ?client_id,
                                        "send entity spawn to client who just gained visibility"
                                    );
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
                                        trace!(
                                            ?entity,
                                            ?client_id,
                                            "send entity spawn to client who maintained visibility"
                                        );
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
                    debug!(?entity, target = ?new_connected_target, "Replicate to newly connected clients");
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

                // TODO: if replicate is removed and re-added, we would spawn a new entity!
                //  in that case we might want to use reuse the remote entity
                // only try to replicate if the replicate component was just added
                if replicate.is_added() {
                    trace!(?entity, "send entity spawn");
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
pub(crate) fn send_component_update<C: Component, R: ReplicationSend>(
    registry: Res<ComponentRegistry>,
    query: Query<(Entity, Ref<C>, Ref<Replicate>, Option<&ReplicateVisibility>)>,
    system_bevy_ticks: SystemChangeTick,
    mut sender: ResMut<R>,
) {
    let kind = registry.net_id::<C>();
    query.iter().for_each(|(entity, component, replicate, visibility)| {
        // do not replicate components that are disabled
        if replicate.is_disabled::<C>() {
            return;
        }
        // will store (NetworkTarget, is_Insert). We use this to avoid serializing if there are no clients we need to replicate to
        let mut replicate_args = vec![];
        match replicate.replication_mode {
            ReplicationMode::Room => {
                visibility.unwrap().clients_cache
                    .iter()
                    .for_each(|(client_id, visibility)| {
                        if replicate.replication_target.should_send_to(client_id) {
                            let target = replicate.target::<C>(NetworkTarget::Only(vec![*client_id]));
                            match visibility {
                                // TODO: here we required the component to be clone because we send it to multiple clients.
                                //  but maybe we can instead serialize it to Bytes early and then have the bytes be shared between clients?
                                //  or just pass a reference?
                                ClientVisibility::Gained => {
                                    replicate_args.push((target, true));
                                }
                                ClientVisibility::Lost => {}
                                ClientVisibility::Maintained => {
                                    // send a component_insert for components that were newly added
                                    if component.is_added() {
                                        replicate_args.push((target, true));
                                    } else {
                                        // only update components that were not newly added

                                        // do not send updates for these components, only inserts/removes
                                        if replicate.is_replicate_once::<C>() {
                                            // we can exit the function immediately because we know we don't want to replicate
                                            // to any client
                                            return;
                                        }
                                        replicate_args.push((target, true));
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
                    replicate_args.push((replicate.target::<C>(new_connected_target), true));
                    // don't re-send to newly connection client
                    target.exclude(new_connected_clients.clone());
                }

                let target = replicate.target::<C>(target);
                // send a component_insert for components that were newly added
                // or if replicate was newly added.
                // TODO: ideally what we should be checking is: is the component newly added
                //  for the client we are sending to?
                //  Otherwise another solution would be to also insert the component on ComponentUpdate if it's missing
                //  Or should we just have ComponentInsert and ComponentUpdate be the same thing? Or we check
                //  on the receiver's entity world mut to know if we emit a ComponentInsert or a ComponentUpdate?
                if component.is_added() || replicate.is_added() {
                    trace!("component is added");
                    replicate_args.push((target, true));
                } else {
                    // do not send updates for these components, only inserts/removes
                    if replicate.is_replicate_once::<C>() {
                        trace!(?entity,
                            "not replicating updates for {:?} because it is marked as replicate_once",
                            kind
                        );
                        // we can exit the function immediately because we know we don't want to replicate
                        // to any client
                        return;
                    }
                    // otherwise send an update for all components that changed since the
                    // last update we have ack-ed
                    replicate_args.push((target, false));
                }
            }
        }

        if !replicate_args.is_empty() {
            // serialize component
            let writer = sender.writer();
            let raw_data = registry.serialize(component.as_ref(), writer).expect("Could not serialize component");

            replicate_args.into_iter().for_each(|(target, is_insert)| {
                if is_insert {
                    let _ = sender
                        .prepare_component_insert(
                            entity,
                            kind,
                            // TODO: avoid the clone by using Arc<u8>?
                            raw_data.clone(),
                            replicate.as_ref(),
                            target,
                            system_bevy_ticks.this_run(),
                        )
                        .map_err(|e| {
                            error!("error sending component insert: {:?}", e);
                        });
                } else {
                    let _ = sender
                        .prepare_component_update(
                            entity,
                            kind,
                            raw_data.clone(),
                            replicate.as_ref(),
                            target,
                            component.last_changed(),
                            system_bevy_ticks.this_run(),
                        )
                        .map_err(|e| {
                            error!("error sending component update: {:?}", e);
                        });
                }
            });
        }
    });
}

/// This system sends updates for all components that were removed
pub(crate) fn send_component_removed<C: Component, R: ReplicationSend>(
    registry: Res<ComponentRegistry>,
    // only remove the component for entities that are being actively replicated
    query: Query<(&Replicate, Option<&ReplicateVisibility>)>,
    system_bevy_ticks: SystemChangeTick,
    mut removed: RemovedComponents<C>,
    mut sender: ResMut<R>,
) {
    let kind = registry.net_id::<C>();
    removed.read().for_each(|entity| {
        if let Ok((replicate, visibility)) = query.get(entity) {
            // do not replicate components that are disabled
            if replicate.is_disabled::<C>() {
                return;
            }
            match replicate.replication_mode {
                ReplicationMode::Room => {
                    visibility
                        .unwrap()
                        .clients_cache
                        .iter()
                        .for_each(|(client_id, visibility)| {
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
                        })
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

/// add replication systems that are shared between client and server
pub(crate) fn add_replication_send_systems<R: ReplicationSend>(app: &mut App) {
    // we need to add despawn trackers immediately for entities for which we add replicate
    app.add_systems(
        PreUpdate,
        handle_replicate_add::<R>.after(ServerReplicationSet::ClientReplication),
    );
    app.add_systems(
        PostUpdate,
        (
            // TODO: try to move this to ReplicationSystems as well? entities are spawned only once
            //  so we can run the system every frame
            //  putting it here means we might miss entities that are spawned and depspawned within the send_interval? bug or feature?
            send_entity_spawn::<R>
                .in_set(InternalReplicationSet::<R::SetMarker>::BufferEntityUpdates),
            // NOTE: we need to run `send_entity_despawn` once per frame (and not once per send_interval)
            //  because the RemovedComponents Events are present only for 1 frame and we might miss them if we don't run this every frame
            //  It is ok to run it every frame because it creates at most one message per despawn
            // NOTE: we make sure to update the replicate_cache before we make use of it in `send_entity_despawn`
            (handle_replicate_add::<R>, handle_replicate_remove::<R>)
                .in_set(InternalReplicationSet::<R::SetMarker>::HandleReplicateUpdate),
            send_entity_despawn::<R>
                .in_set(InternalReplicationSet::<R::SetMarker>::BufferDespawnsAndRemovals),
        ),
    );
}

pub(crate) fn register_replicate_component_send<C: Component, R: ReplicationSend>(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (
            // NOTE: we need to run `send_component_removed` once per frame (and not once per send_interval)
            //  because the RemovedComponents Events are present only for 1 frame and we might miss them if we don't run this every frame
            //  It is ok to run it every frame because it creates at most one message per despawn
            crate::shared::replication::systems::send_component_removed::<C, R>
                .in_set(InternalReplicationSet::<R::SetMarker>::BufferDespawnsAndRemovals),
            // NOTE: we run this system once every `send_interval` because we don't want to send too many Update messages
            //  and use up all the bandwidth
            crate::shared::replication::systems::send_component_update::<C, R>
                .in_set(InternalReplicationSet::<R::SetMarker>::BufferComponentUpdates),
        ),
    );
}

pub(crate) fn cleanup<R: ReplicationSend>(mut sender: ResMut<R>, tick_manager: Res<TickManager>) {
    let tick = tick_manager.tick();
    sender.cleanup(tick);
}

#[cfg(test)]
mod tests {
    // TODO: how to check that no despawn message is sent?
    // /// Check that when replicated entities in other rooms than the current client are despawned,
    // /// the despawn is not sent to the client
    // #[test]
    // fn test_other_rooms_despawn() {
    //     let mut stepper = BevyStepper::default();
    //
    //     let server_entity = stepper
    //         .server_app
    //         .world
    //         .spawn((
    //             Replicate {
    //                 replication_mode: ReplicationMode::Room,
    //                 ..default()
    //             },
    //             Component1(0.0),
    //         ))
    //         .id();
    //     let mut room_manager = stepper.server_app.world.resource_mut::<RoomManager>();
    //     room_manager.add_client(ClientId::Netcode(TEST_CLIENT_ID), RoomId(0));
    //     room_manager.add_entity(server_entity, RoomId(0));
    //     stepper.frame_step();
    //     stepper.frame_step();
    //
    //     // check that the entity was replicated
    //     let client_entity = stepper
    //         .client_app
    //         .world
    //         .query_filtered::<Entity, With<Component1>>()
    //         .single(&stepper.client_app.world);
    //
    //     // update the room of the server entity to not be in the client's room anymore
    //     stepper
    //         .server_app
    //         .world
    //         .resource_mut::<RoomManager>()
    //         .remove_entity(server_entity, RoomId(0));
    //     stepper.frame_step();
    //     stepper.frame_step();
    //
    //     // despawn the entity
    //     stepper.server_app.world.entity_mut(server_entity).despawn();
    //     stepper.frame_step();
    //     stepper.frame_step();
    //
    //     // the despawn shouldn't be replicated to the client, since it's in a different room
    //     assert!(stepper.client_app.world.get_entity(client_entity).is_some());
    // }
}
