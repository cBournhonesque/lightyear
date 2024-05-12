//! Bevy [`bevy::prelude::System`]s used for replication
use std::any::TypeId;
use std::ops::Deref;

use bevy::ecs::entity::Entities;
use bevy::ecs::system::SystemChangeTick;
use bevy::prelude::{
    Added, App, Changed, Commands, Component, DetectChanges, Entity, IntoSystemConfigs, Mut,
    PostUpdate, PreUpdate, Query, Ref, RemovedComponents, Res, ResMut, With, Without,
};
use tracing::{debug, error, info, trace, warn};

use crate::prelude::server::ConnectionManager;
use crate::prelude::{ClientId, ReplicationGroup, ShouldBePredicted, TargetEntity, TickManager};
use crate::protocol::component::{ComponentNetId, ComponentRegistry};
use crate::serialize::RawData;
use crate::server::replication::ServerReplicationSet;
use crate::server::visibility::immediate::{ClientVisibility, ReplicateVisibility};
use crate::shared::replication::components::{
    DespawnTracker, Replicate, ReplicationGroupId, ReplicationTarget, VisibilityMode,
};
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::replication::{ReplicationReceive, ReplicationSend};
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};

// TODO: replace this with observers
/// Metadata that holds Replicate-information (so that when the entity is despawned we know
/// how to replicate the despawn)
pub(crate) struct ReplicateCache {
    replication_target: NetworkTarget,
    replication_group: ReplicationGroup,
    replication_mode: VisibilityMode,
    /// If mode = Room, the list of clients that could see the entity
    pub(crate) replication_clients_cache: Vec<ClientId>,
}

/// For every entity that removes their Replicate component but are not despawned, remove the component
/// from our replicate cache (so that the entity's despawns are no longer replicated)
pub(crate) fn handle_replicate_remove<R: ReplicationSend>(
    mut commands: Commands,
    mut sender: ResMut<R>,
    mut query: RemovedComponents<Replicate>,
    entity_check: &Entities,
) {
    for entity in query.read() {
        if entity_check.contains(entity) {
            debug!("handling replicate component remove (delete from cache)");
            sender.get_mut_replicate_cache().remove(&entity);
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
    // We use `(With<Replicate>, Without<DespawnTracker>)` as an optimization to
    // only get the subset of entities that have had Replicate added
    // (`Added<Replicate>` queries through each entity that has `Replicate`)
    query: Query<
        (
            Entity,
            &ReplicationTarget,
            &ReplicationGroup,
            &VisibilityMode,
        ),
        (With<ReplicationTarget>, Without<DespawnTracker>),
    >,
) {
    for (entity, replication_target, group, visibility_mode) in query.iter() {
        debug!("Replicate component was added for entity {entity:?}");
        commands.entity(entity).insert(DespawnTracker);
        let despawn_metadata = ReplicateCache {
            replication_target: replication_target.replication.clone(),
            replication_group: *group,
            replication_mode: *visibility_mode,
            replication_clients_cache: vec![],
        };
        sender
            .get_mut_replicate_cache()
            .insert(entity, despawn_metadata);
        if visibility_mode == &VisibilityMode::InterestManagement {
            debug!("Adding ReplicateVisibility component for entity {entity:?}");
            commands
                .entity(entity)
                .insert(ReplicateVisibility::default());
        }
    }
}

// TODO: also send despawn if the target changed?
pub(crate) fn send_entity_despawn<R: ReplicationSend>(
    query: Query<(
        Entity,
        &ReplicationTarget,
        &ReplicationGroup,
        &ReplicateVisibility,
    )>,
    // TODO: ideally we want to send despawns for entities that still had REPLICATE at the time of despawn
    //  not just entities that had despawn tracker once
    mut despawn_removed: RemovedComponents<DespawnTracker>,
    mut sender: ResMut<R>,
) {
    // Send entity-despawn for entities that still exist for clients that lost visibility
    query
        .iter()
        .for_each(|(entity, replication_target, group, visibility)| {
            // no need to check if the visibility mode is InterestManagement, because the ReplicateVisibility component
            // is only present if that is the case
            let target = visibility
                .clients_cache
                .iter()
                .filter_map(|(client_id, visibility)| {
                    if replication_target.targets(client_id)
                        && matches!(visibility, ClientVisibility::Lost)
                    {
                        debug!(
                            "sending entity despawn for entity: {:?} because ClientVisibility::Lost",
                            entity
                        );
                        return Some(*client_id);

                    }
                    return None
                }).collect();

            if !target.is_empty() {
                let _ = sender
                    .prepare_entity_despawn(
                        entity,
                        group,
                        target
                    )
                    .inspect_err(|e| {
                        error!("error sending entity despawn: {:?}", e);
                    });
            }

        });

    // Despawn entities when the entity gets despawned on local world
    for entity in despawn_removed.read() {
        trace!("DespawnTracker component got removed, preparing entity despawn message!");
        // TODO: we still don't want to replicate the despawn if the entity was not in the same room as the client!
        // only replicate the despawn if the entity still had a Replicate component
        if let Some(replicate_cache) = sender.get_mut_replicate_cache().remove(&entity) {
            // TODO: DO NOT SEND ENTITY DESPAWN TO THE CLIENT WHO JUST DISCONNECTED!
            let mut network_target = replicate_cache.replication_target;

            // TODO: for this to work properly, we need the replicate stored in `sender.get_mut_replicate_component_cache()`
            //  to be updated for every replication change! Wait for observers instead.
            //  How did it work on the `main` branch? was there something else making it work? Maybe the
            //  update replicate ran before
            if replicate_cache.replication_mode == VisibilityMode::InterestManagement {
                // if the mode was room, only replicate the despawn to clients that were in the same room
                network_target.intersection(NetworkTarget::Only(
                    replicate_cache.replication_clients_cache,
                ));
            }
            trace!(?entity, ?network_target, "send entity despawn");
            let group_id = replicate_cache.replication_group.group_id(Some(entity));
            let _ = sender
                .prepare_entity_despawn(entity, group_id, network_target)
                // TODO: bubble up errors to user via ConnectionEvents?
                .inspect_err(|e| {
                    error!("error sending entity despawn: {:?}", e);
                });
        }
    }
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
    query: Query<(
        Entity,
        Ref<C>,
        Ref<ReplicationTarget>,
        &ReplicationGroup,
        Option<&ReplicateVisibility>,
    )>,
    system_bevy_ticks: SystemChangeTick,
    mut sender: ResMut<R>,
) {
    let kind = registry.net_id::<C>();
    query
        .iter()
        .for_each(|(entity, component, replication_target, group, visibility)| {
            // TODO: READD THIS
            // // do not replicate components that are disabled
            // if replicate.is_disabled::<C>() {
            //     return;
            // }

            let (insert_target, update_target): (NetworkTarget, NetworkTarget) = match visibility {
                Some(visibility) => {
                    let mut insert_clients = vec![];
                    let mut update_clients = vec![];
                    visibility
                        .clients_cache
                        .iter()
                        .for_each(|(client_id, visibility)| {
                            if replication_target.replication.targets(client_id) {
                                // TODO: RE-ADD CUSTOM TARGET
                                match visibility {
                                    ClientVisibility::Gained => {
                                        insert_clients.push(*client_id);
                                    }
                                    ClientVisibility::Lost => {}
                                    ClientVisibility::Maintained => {
                                        // send a component_insert for components that were newly added
                                        if component.is_added() {
                                            insert_clients.push(*client_id);
                                        } else {
                                            // only update components that were not newly added

                                            // TODO: readd-this
                                            // // do not send updates for these components, only inserts/removes
                                            // if replicate.is_replicate_once::<C>() {
                                            //     // we can exit the function immediately because we know we don't want to replicate
                                            //     // to any client
                                            //     return;
                                            // }
                                            update_clients.push(*client_id);
                                        }
                                    }
                                }
                            }
                        });
                    (NetworkTarget::from(insert_clients), NetworkTarget::from(update_clients))
                }
                None => {
                    let (mut insert_target, mut update_target) =
                        (NetworkTarget::None, NetworkTarget::None);

                    // send a component_insert for components that were newly added
                    // or if replicate was newly added.
                    // TODO: ideally what we should be checking is: is the component newly added
                    //  for the client we are sending to?
                    //  Otherwise another solution would be to also insert the component on ComponentUpdate if it's missing
                    //  Or should we just have ComponentInsert and ComponentUpdate be the same thing? Or we check
                    //  on the receiver's entity world mut to know if we emit a ComponentInsert or a ComponentUpdate?
                    if component.is_added() || replication_target.is_added() {
                        trace!("component is added or replication_target is added");
                        insert_target.union(&replication_target.replication);
                    } else {
                        // TODO: re-add this
                        // // do not send updates for these components, only inserts/removes
                        // if replicate.is_replicate_once::<C>() {
                        //     trace!(?entity,
                        //         "not replicating updates for {:?} because it is marked as replicate_once",
                        //         kind
                        //     );
                        //     // we can exit the function immediately because we know we don't want to replicate
                        //     // to any client
                        //     return;
                        // }

                        // otherwise send an update for all components that changed since the
                        // last update we have ack-ed
                        update_target.union(&replication_target.replication);
                    }

                    let new_connected_clients = sender.new_connected_clients();
                    // replicate all components to newly connected clients
                    if !new_connected_clients.is_empty() {
                        // replicate to the newly connected clients that match our target
                        let mut new_connected_target = NetworkTarget::Only(new_connected_clients);
                        new_connected_target.intersection(&replication_target.replication);
                        debug!(?entity, target = ?new_connected_target, "Replicate to newly connected clients");
                        update_target.union(&new_connected_target);
                    }
                    (insert_target, update_target)
                }
            };

            if !insert_target.is_empty() || !update_target.is_empty() {
                // serialize component
                let writer = sender.writer();
                let raw_data = registry
                    .serialize(component.as_ref(), writer)
                    .expect("Could not serialize component");

                if !insert_target.is_empty() {
                    let _ = sender
                        .prepare_component_insert(
                            entity,
                            kind,
                            // TODO: avoid the clone by using Arc<u8>?
                            raw_data.clone(),
                            &registry,
                            replication_target.as_ref(),
                            &group,
                            insert_target
                        )
                        .inspect_err(|e| {
                            error!("error sending component insert: {:?}", e);
                        });
                }
                if !update_target.is_empty() {
                    let _ = sender
                        .prepare_component_update(
                            entity,
                            kind,
                            raw_data,
                            &group,
                            update_target,
                            component.last_changed(),
                            system_bevy_ticks.this_run(),
                        )
                        .inspect_err(|e| {
                            error!("error sending component update: {:?}", e);
                        });
                }
            }
        });
}

/// This system sends updates for all components that were removed
pub(crate) fn send_component_removed<C: Component, R: ReplicationSend>(
    registry: Res<ComponentRegistry>,
    // only remove the component for entities that are being actively replicated
    query: Query<(
        &ReplicationTarget,
        &ReplicationGroup,
        Option<&ReplicateVisibility>,
    )>,
    mut removed: RemovedComponents<C>,
    mut sender: ResMut<R>,
) {
    let kind = registry.net_id::<C>();
    removed.read().for_each(|entity| {
        if let Ok((replication_target, group, visibility)) = query.get(entity) {
            // TODO: re-add this!
            // // do not replicate components that are disabled
            // if replicate.is_disabled::<C>() {
            //     return;
            // }
            let target = match visibility {
                Some(visibility) => {
                    visibility
                        .clients_cache
                        .iter()
                        .filter_map(|(client_id, visibility)| {
                            if replication_target.replication.targets(client_id) {
                                // TODO: maybe send no matter the vis?
                                if matches!(visibility, ClientVisibility::Maintained) {
                                    // TODO: USE THE CUSTOM REPLICATE TARGET FOR THIS COMPONENT IF PRESENT!
                                    return Some(*client_id);
                                }
                            };
                            return None;
                        })
                        .collect()
                }
                None => {
                    trace!("sending component remove!");
                    // TODO: USE THE CUSTOM REPLICATE TARGET FOR THIS COMPONENT IF PRESENT!
                    replication_target.replication.clone()
                }
            };
            if target.is_empty() {
                return;
            }
            let group_id = group.group_id(Some(entity));
            debug!(?entity, ?kind, "Sending RemoveComponent");
            let _ = sender.prepare_component_remove(entity, kind, group, target);
        }
    })
}

pub(crate) fn register_replicate_component_send<C: Component, R: ReplicationSend>(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (
            // NOTE: we need to run `send_component_removed` once per frame (and not once per send_interval)
            //  because the RemovedComponents Events are present only for 1 frame and we might miss them if we don't run this every frame
            //  It is ok to run it every frame because it creates at most one message per despawn
            send_component_removed::<C, R>
                .in_set(InternalReplicationSet::<R::SetMarker>::BufferDespawnsAndRemovals),
            // NOTE: we run this system once every `send_interval` because we don't want to send too many Update messages
            //  and use up all the bandwidth
            send_component_update::<C, R>
                .in_set(InternalReplicationSet::<R::SetMarker>::BufferComponentUpdates),
        ),
    );
}

/// Systems that runs internal clean-up on the ReplicationSender
/// (handle tick wrapping, etc.)
pub(crate) fn send_cleanup<R: ReplicationSend>(
    mut sender: ResMut<R>,
    tick_manager: Res<TickManager>,
) {
    let tick = tick_manager.tick();
    sender.cleanup(tick);
}

/// Systems that runs internal clean-up on the ReplicationReceiver
/// (handle tick wrapping, etc.)
pub(crate) fn receive_cleanup<R: ReplicationReceive>(
    mut receiver: ResMut<R>,
    tick_manager: Res<TickManager>,
) {
    let tick = tick_manager.tick();
    receiver.cleanup(tick);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::client;
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};

    #[test]
    fn test_entity_spawn() {
        let mut stepper = BevyStepper::default();

        // 1. spawn an entity with visibility::All
        let entity = stepper
            .server_app
            .world
            .spawn((Replicate::default(), Component1(0.0)))
            .id();
        stepper.frame_step();
        stepper.frame_step();

        assert!(stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(entity)
            .is_some());
    }

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
