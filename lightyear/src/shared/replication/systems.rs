//! Bevy [`bevy::prelude::System`]s used for replication
use std::any::TypeId;
use std::ops::Deref;

use bevy::ecs::entity::Entities;
use bevy::ecs::system::SystemChangeTick;
use bevy::prelude::{
    Added, App, Changed, Commands, Component, DetectChanges, Entity, Has, IntoSystemConfigs, Mut,
    PostUpdate, PreUpdate, Query, Ref, RemovedComponents, Res, ResMut, With, Without,
};
use tracing::{debug, error, info, trace, warn};

use crate::prelude::{ClientId, ReplicationGroup, ShouldBePredicted, TargetEntity, TickManager};
use crate::protocol::component::{ComponentNetId, ComponentRegistry};
use crate::serialize::RawData;
use crate::server::replication::ServerReplicationSet;
use crate::server::visibility::immediate::{ClientVisibility, ReplicateVisibility};
use crate::shared::replication::components::{
    DespawnTracker, DisabledComponent, OverrideTargetComponent, Replicate, ReplicateOnceComponent,
    ReplicationGroupId, ReplicationTarget, VisibilityMode,
};
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::replication::{ReplicationReceive, ReplicationSend};
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};

// TODO: replace this with observers
/// Metadata that holds Replicate-information from the previous send_interval's replication.
/// - when the entity gets despawned, we will use this to know how to replicate the despawn
/// - when the replicate metadata changes, we will use this to compute diffs
#[derive(PartialEq, Debug)]
pub(crate) struct ReplicateCache {
    pub(crate) replication_target: NetworkTarget,
    pub(crate) replication_group: ReplicationGroup,
    pub(crate) visibility_mode: VisibilityMode,
    /// If mode = Room, the list of clients that could see the entity
    pub(crate) replication_clients_cache: Vec<ClientId>,
}

/// For every entity that removes their ReplicationTarget component but are not despawned, remove the component
/// from our replicate cache (so that the entity's despawns are no longer replicated)
pub(crate) fn handle_replicate_remove<R: ReplicationSend>(
    // mut commands: Commands,
    mut sender: ResMut<R>,
    mut query: RemovedComponents<ReplicationTarget>,
    entity_check: &Entities,
) {
    for entity in query.read() {
        if entity_check.contains(entity) {
            debug!("handling replicate component remove (delete from cache)");
            sender.get_mut_replicate_cache().remove(&entity);
            // TODO: should we also remove the replicate-visibility? or should we keep it?
            // commands.entity(entity).remove::<ReplicateVisibility>();
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
            visibility_mode: *visibility_mode,
            replication_clients_cache: vec![],
        };
        sender
            .get_mut_replicate_cache()
            .insert(entity, despawn_metadata);
    }
}

/// Update the replication_target in the cache when the ReplicationTarget component changes
pub(crate) fn handle_replication_target_update<R: ReplicationSend>(
    mut sender: ResMut<R>,
    target_query: Query<
        (Entity, Ref<ReplicationTarget>),
        (Changed<ReplicationTarget>, With<DespawnTracker>),
    >,
) {
    for (entity, replication_target) in target_query.iter() {
        if replication_target.is_changed() && !replication_target.is_added() {
            if let Some(replicate_cache) = sender.get_mut_replicate_cache().get_mut(&entity) {
                replicate_cache.replication_target = replication_target.replication.clone();
            }
        }
    }
}

// TODO: also send despawn if the target changed?
pub(crate) fn send_entity_despawn<R: ReplicationSend>(
    query: Query<(
        Entity,
        Ref<ReplicationTarget>,
        &ReplicationGroup,
        Option<&ReplicateVisibility>,
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
            let mut target: NetworkTarget = match visibility {
                Some(visibility) => {
                    // send despawn for clients that lost visibility
                    visibility
                        .clients_cache
                        .iter()
                        .filter_map(|(client_id, visibility)| {
                            if replication_target.replication.targets(client_id)
                                && matches!(visibility, ClientVisibility::Lost) {
                                debug!(
                                    "sending entity despawn for entity: {:?} because ClientVisibility::Lost",
                                    entity
                                );
                                return Some(*client_id);
                            }
                            None
                        }).collect()
                }
                None => {
                    NetworkTarget::None
                }
            };
            // if the replication target changed, find the clients that were removed in the new replication target
            if replication_target.is_changed() && !replication_target.is_added() {
                if let Some(cache) = sender.get_mut_replicate_cache().get_mut(&entity) {
                    let mut new_despawn = cache.replication_target.clone();
                    new_despawn.exclude(&replication_target.replication);
                    target.union(&new_despawn);
                }
            }
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
            if replicate_cache.visibility_mode == VisibilityMode::InterestManagement {
                // if the mode was room, only replicate the despawn to clients that were in the same room
                network_target.intersection(&NetworkTarget::Only(
                    replicate_cache.replication_clients_cache,
                ));
            }
            trace!(?entity, ?network_target, "send entity despawn");
            let _ = sender
                .prepare_entity_despawn(entity, &replicate_cache.replication_group, network_target)
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
        Has<DisabledComponent<C>>,
        Has<ReplicateOnceComponent<C>>,
        Option<&OverrideTargetComponent<C>>,
    )>,
    system_bevy_ticks: SystemChangeTick,
    mut sender: ResMut<R>,
) {
    let kind = registry.net_id::<C>();
    query
        .iter()
        .for_each(|(entity, component, replication_target, group, visibility, disabled, replicate_once, override_target)| {
            // do not replicate components that are disabled
            if disabled {
                return;
            }
            // use the overriden target if present
            let target = override_target.map_or(&replication_target.replication, |override_target| &override_target.target);
            let (insert_target, update_target): (NetworkTarget, NetworkTarget) = match visibility {
                Some(visibility) => {
                    let mut insert_clients = vec![];
                    let mut update_clients = vec![];
                    visibility
                        .clients_cache
                        .iter()
                        .for_each(|(client_id, visibility)| {
                            if target.targets(client_id) {
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
                                            // for components that were not newly added, only send as updates
                                            if replicate_once {
                                                // we can exit the function immediately because we know we don't want to replicate
                                                // to any client
                                                return;
                                            }
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
                        insert_target.union(target);
                    } else {
                        // do not send updates for these components, only inserts/removes
                        if replicate_once {
                            trace!(?entity,
                                "not replicating updates for {:?} because it is marked as replicate_once",
                                kind
                            );
                            return;
                        }
                        // otherwise send an update for all components that changed since the
                        // last update we have ack-ed
                        update_target.union(target);
                    }

                    let new_connected_clients = sender.new_connected_clients();
                    // replicate all components to newly connected clients
                    if !new_connected_clients.is_empty() {
                        // replicate to the newly connected clients that match our target
                        let mut new_connected_target = NetworkTarget::Only(new_connected_clients);
                        new_connected_target.intersection(target);
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
                            group,
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
                            group,
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
        Has<DisabledComponent<C>>,
        Option<&OverrideTargetComponent<C>>,
    )>,
    mut removed: RemovedComponents<C>,
    mut sender: ResMut<R>,
) {
    let kind = registry.net_id::<C>();
    removed.read().for_each(|entity| {
        if let Ok((replication_target, group, visibility, disabled, override_target)) =
            query.get(entity)
        {
            // do not replicate components that are disabled
            if disabled {
                return;
            }
            // use the overriden target if present
            let base_target = override_target
                .map_or(&replication_target.replication, |override_target| {
                    &override_target.target
                });
            let target = match visibility {
                Some(visibility) => {
                    visibility
                        .clients_cache
                        .iter()
                        .filter_map(|(client_id, visibility)| {
                            if base_target.targets(client_id) {
                                // TODO: maybe send no matter the vis?
                                if matches!(visibility, ClientVisibility::Maintained) {
                                    // TODO: USE THE CUSTOM REPLICATE TARGET FOR THIS COMPONENT IF PRESENT!
                                    return Some(*client_id);
                                }
                            };
                            None
                        })
                        .collect()
                }
                None => {
                    trace!("sending component remove!");
                    // TODO: USE THE CUSTOM REPLICATE TARGET FOR THIS COMPONENT IF PRESENT!
                    base_target.clone()
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
    use crate::prelude::client::Confirmed;
    use crate::prelude::server::VisibilityManager;
    use crate::prelude::{client, server, Replicated};
    use crate::shared::replication::components::{Controlled, ControlledBy};
    use crate::tests::multi_stepper::{MultiBevyStepper, TEST_CLIENT_ID_1, TEST_CLIENT_ID_2};
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step, TEST_CLIENT_ID};
    use bevy::prelude::default;

    // TODO: test entity spawn newly connected client
    // TODO: test entity spawn replication target was updated

    #[test]
    fn test_entity_spawn() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server with visibility::All
        let server_entity = stepper.server_app.world.spawn_empty().id();
        stepper.frame_step();
        stepper.frame_step();
        // check that entity wasn't spawned
        assert!(stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .is_none());

        // add replicate
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert(Replicate {
                target: ReplicationTarget {
                    replication: NetworkTarget::All,
                    prediction: NetworkTarget::All,
                    interpolation: NetworkTarget::All,
                },
                controlled_by: ControlledBy {
                    target: NetworkTarget::All,
                },
                ..default()
            });

        stepper.frame_step();
        stepper.frame_step();

        // check that the entity was spawned
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
        // check that prediction, interpolation, controlled was handled correctly
        let confirmed = stepper
            .client_app
            .world
            .entity(client_entity)
            .get::<Confirmed>()
            .expect("Confirmed component missing");
        assert!(confirmed.predicted.is_some());
        assert!(confirmed.interpolated.is_some());
        assert!(stepper
            .client_app
            .world
            .entity(client_entity)
            .get::<Controlled>()
            .is_some());
    }

    #[test]
    fn test_entity_spawn_visibility() {
        let mut stepper = MultiBevyStepper::default();

        // spawn an entity on server with visibility::InterestManagement
        let server_entity = stepper
            .server_app
            .world
            .spawn(Replicate {
                visibility: VisibilityMode::InterestManagement,
                ..default()
            })
            .id();
        stepper.frame_step();
        stepper.frame_step();

        // check that entity wasn't spawned
        assert!(stepper
            .client_app_1
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .is_none());
        // make entity visible
        stepper
            .server_app
            .world
            .resource_mut::<VisibilityManager>()
            .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID_1), server_entity);
        stepper.frame_step();
        stepper.frame_step();

        // check that entity was spawned
        let client_entity = *stepper
            .client_app_1
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
        // check that the entity was not spawned on the other client
        assert!(stepper
            .client_app_2
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .is_none());
    }

    #[test]
    fn test_entity_spawn_preexisting_target() {
        let mut stepper = BevyStepper::default();

        let client_entity = stepper.client_app.world.spawn_empty().id();
        stepper.frame_step();
        let server_entity = stepper
            .server_app
            .world
            .spawn((
                Replicate::default(),
                TargetEntity::Preexisting(client_entity),
            ))
            .id();
        stepper.frame_step();
        stepper.frame_step();

        // check that the entity was replicated on the client entity
        assert_eq!(
            stepper
                .client_app
                .world
                .resource::<client::ConnectionManager>()
                .replication_receiver
                .remote_entity_map
                .get_local(server_entity)
                .unwrap(),
            &client_entity
        );
        assert!(stepper
            .client_app
            .world
            .get::<Replicated>(client_entity)
            .is_some());
        assert_eq!(stepper.client_app.world.entities().len(), 1);
    }

    /// Check that if we change the replication target on an entity that already has one
    /// we spawn the entity for new clients
    #[test]
    fn test_entity_spawn_replication_target_update() {
        let mut stepper = MultiBevyStepper::default();

        // spawn an entity on server to client 1
        let server_entity = stepper
            .server_app
            .world
            .spawn(Replicate {
                target: ReplicationTarget {
                    replication: NetworkTarget::Single(ClientId::Netcode(TEST_CLIENT_ID_1)),
                    ..default()
                },
                ..default()
            })
            .id();
        stepper.frame_step();
        stepper.frame_step();

        let client_entity_1 = *stepper
            .client_app_1
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client 1");

        // update the replication target
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert(ReplicationTarget {
                replication: NetworkTarget::All,
                ..default()
            });
        stepper.frame_step();
        stepper.frame_step();

        // check that the entity gets replicated to the other client
        stepper
            .client_app_2
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client 2");
        // TODO: check that client 1 did not receive another entity-spawn message
    }

    #[test]
    fn test_entity_despawn() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper.server_app.world.spawn(Replicate::default()).id();
        stepper.frame_step();
        stepper.frame_step();

        // check that the entity was spawned
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // despawn
        stepper.server_app.world.despawn(server_entity);
        stepper.frame_step();
        stepper.frame_step();

        // check that the entity was despawned
        assert!(stepper.client_app.world.get_entity(client_entity).is_none());
    }

    /// Check that if interest management is used, a client losing visibility of an entity
    /// will cause the server to send a despawn-entity message to the client
    #[test]
    fn test_entity_despawn_lose_visibility() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn(Replicate {
                visibility: VisibilityMode::InterestManagement,
                ..default()
            })
            .id();
        stepper
            .server_app
            .world
            .resource_mut::<VisibilityManager>()
            .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID), server_entity);

        stepper.frame_step();
        stepper.frame_step();

        // check that the entity was spawned
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // lose visibility
        stepper
            .server_app
            .world
            .resource_mut::<VisibilityManager>()
            .lose_visibility(ClientId::Netcode(TEST_CLIENT_ID), server_entity);
        stepper.frame_step();
        stepper.frame_step();

        // check that the entity was despawned
        assert!(stepper.client_app.world.get_entity(client_entity).is_none());
    }

    /// Test that if an entity with visibility is despawned, the despawn-message is not sent
    /// to other clients who do not have visibility of the entity
    #[test]
    fn test_entity_despawn_non_visible() {
        let mut stepper = MultiBevyStepper::default();

        // spawn one entity replicated to each client
        // they will share the same replication group id, so that each client's ReplicationReceiver
        // can read the replication messages of the other client
        let server_entity_1 = stepper
            .server_app
            .world
            .spawn(Replicate {
                visibility: VisibilityMode::InterestManagement,
                group: ReplicationGroup::new_id(1),
                ..default()
            })
            .id();
        let server_entity_2 = stepper
            .server_app
            .world
            .spawn(Replicate {
                visibility: VisibilityMode::InterestManagement,
                group: ReplicationGroup::new_id(1),
                ..default()
            })
            .id();
        stepper
            .server_app
            .world
            .resource_mut::<VisibilityManager>()
            .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID_1), server_entity_1)
            .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID_2), server_entity_2);
        stepper.frame_step();
        stepper.frame_step();

        // check that the entity was spawned on each client
        let client_entity_1 = *stepper
            .client_app_1
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity_1)
            .expect("entity was not replicated to client 1");
        let client_entity_2 = *stepper
            .client_app_2
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity_2)
            .expect("entity was not replicated to client 2");

        // update the entity_map on client 2 to re-use the same server entity as client 1
        // so that replication messages for server_entity_1 could also be read by client 2
        stepper
            .client_app_2
            .world
            .resource_mut::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .insert(server_entity_1, client_entity_2);

        // despawn the server_entity_1
        stepper.server_app.world.despawn(server_entity_1);
        stepper.frame_step();
        stepper.frame_step();

        // check that the entity was despawned on client 1
        assert!(stepper
            .client_app_1
            .world
            .get_entity(client_entity_1)
            .is_none());

        // check that the entity still exists on client 2
        assert!(stepper
            .client_app_2
            .world
            .get_entity(client_entity_2)
            .is_some());
    }

    /// Check that if we change the replication target on an entity that already has one
    /// we despawn the entity for new clients
    #[test]
    fn test_entity_despawn_replication_target_update() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server to client 1
        let server_entity = stepper
            .server_app
            .world
            .spawn(Replicate {
                target: ReplicationTarget {
                    replication: NetworkTarget::Single(ClientId::Netcode(TEST_CLIENT_ID)),
                    ..default()
                },
                ..default()
            })
            .id();
        stepper.frame_step();
        stepper.frame_step();

        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // update the replication target
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert(ReplicationTarget {
                replication: NetworkTarget::None,
                ..default()
            });
        stepper.frame_step();
        stepper.frame_step();

        // check that the entity was despawned
        assert!(stepper.client_app.world.get_entity(client_entity).is_none());
    }

    #[test]
    fn test_component_insert() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper.server_app.world.spawn(Replicate::default()).id();
        stepper.frame_step();
        stepper.frame_step();
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // add component
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert(Component1(1.0));
        stepper.frame_step();
        stepper.frame_step();

        // check that the component was replicated
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .expect("component missing"),
            &Component1(1.0)
        );
    }

    #[test]
    fn test_component_insert_visibility_maintained() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn(Replicate {
                visibility: VisibilityMode::InterestManagement,
                ..default()
            })
            .id();
        stepper
            .server_app
            .world
            .resource_mut::<VisibilityManager>()
            .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID), server_entity);
        stepper.frame_step();
        stepper.frame_step();
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // add component
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert(Component1(1.0));
        stepper.frame_step();
        stepper.frame_step();

        // check that the component was replicated
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .expect("component missing"),
            &Component1(1.0)
        );
    }

    #[test]
    fn test_component_insert_visibility_gained() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn(Replicate {
                visibility: VisibilityMode::InterestManagement,
                ..default()
            })
            .id();

        stepper.frame_step();
        stepper.frame_step();

        // add component
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert(Component1(1.0));
        stepper
            .server_app
            .world
            .resource_mut::<VisibilityManager>()
            .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID), server_entity);
        stepper.frame_step();
        stepper.frame_step();

        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
        // check that the component was replicated
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .expect("component missing"),
            &Component1(1.0)
        );
    }

    #[test]
    fn test_component_insert_disabled() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper.server_app.world.spawn(Replicate::default()).id();
        stepper.frame_step();
        stepper.frame_step();
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // add component
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert((Component1(1.0), DisabledComponent::<Component1>::default()));
        stepper.frame_step();
        stepper.frame_step();

        // check that the component was not replicated
        assert!(stepper
            .client_app
            .world
            .entity(client_entity)
            .get::<Component1>()
            .is_none());
    }

    #[test]
    fn test_component_override_target() {
        let mut stepper = MultiBevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn((
                Replicate::default(),
                Component1(1.0),
                OverrideTargetComponent::<Component1>::new(NetworkTarget::Single(
                    ClientId::Netcode(TEST_CLIENT_ID_1),
                )),
            ))
            .id();
        stepper.frame_step();
        stepper.frame_step();
        let client_entity_1 = *stepper
            .client_app_1
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
        let client_entity_2 = *stepper
            .client_app_2
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // check that the component was replicated to client 1 only
        assert_eq!(
            stepper
                .client_app_1
                .world
                .entity(client_entity_1)
                .get::<Component1>()
                .expect("component missing"),
            &Component1(1.0)
        );
        assert!(stepper
            .client_app_2
            .world
            .entity(client_entity_2)
            .get::<Component1>()
            .is_none());
    }

    /// Check that override target works even if the entity uses interest management
    /// We still use visibility, but we use `override_target` instead of `replication_target`
    #[test]
    fn test_component_override_target_visibility() {
        let mut stepper = MultiBevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn((
                Replicate {
                    // target is both
                    visibility: VisibilityMode::InterestManagement,
                    ..default()
                },
                Component1(1.0),
                // override target is only client 1
                OverrideTargetComponent::<Component1>::new(NetworkTarget::Single(
                    ClientId::Netcode(TEST_CLIENT_ID_1),
                )),
            ))
            .id();
        // entity is visible to both
        stepper
            .server_app
            .world
            .resource_mut::<VisibilityManager>()
            .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID_1), server_entity)
            .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID_2), server_entity);
        stepper.frame_step();
        stepper.frame_step();
        let client_entity_1 = *stepper
            .client_app_1
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
        let client_entity_2 = *stepper
            .client_app_2
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // check that the component was replicated to client 1 only
        assert_eq!(
            stepper
                .client_app_1
                .world
                .entity(client_entity_1)
                .get::<Component1>()
                .expect("component missing"),
            &Component1(1.0)
        );
        assert!(stepper
            .client_app_2
            .world
            .entity(client_entity_2)
            .get::<Component1>()
            .is_none());
    }

    #[test]
    fn test_component_update() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn((Replicate::default(), Component1(1.0)))
            .id();
        stepper.frame_step();
        stepper.frame_step();
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // add component
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert(Component1(2.0));
        stepper.frame_step();
        stepper.frame_step();

        // check that the component was replicated
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .expect("component missing"),
            &Component1(2.0)
        );
    }

    /// Check that updates are not sent if the `ReplicationTarget` component gets removed.
    /// Check that updates are resumed when the `ReplicationTarget` component gets re-added.
    #[test]
    fn test_component_update_replication_target_removed() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn((Replicate::default(), Component1(1.0)))
            .id();
        stepper.frame_step();
        stepper.frame_step();
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // remove the replication_target component
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert(Component1(2.0))
            .remove::<ReplicationTarget>();
        stepper.frame_step();
        stepper.frame_step();

        // check that the entity still exists on the client, but that the component was not updated
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .expect("component missing"),
            &Component1(1.0)
        );

        // re-add the replication_target component
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert(ReplicationTarget::default());
        stepper.frame_step();
        stepper.frame_step();
        // check that the component gets updated
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .expect("component missing"),
            &Component1(2.0)
        );
    }

    #[test]
    fn test_component_update_disabled() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn((Replicate::default(), Component1(1.0)))
            .id();
        stepper.frame_step();
        stepper.frame_step();
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");

        // add component
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert((Component1(2.0), DisabledComponent::<Component1>::default()));
        stepper.frame_step();
        stepper.frame_step();

        // check that the component was not updated
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .expect("component missing"),
            &Component1(1.0)
        );
    }

    #[test]
    fn test_component_update_replicate_once() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn((
                Replicate::default(),
                Component1(1.0),
                ReplicateOnceComponent::<Component1>::default(),
            ))
            .id();
        stepper.frame_step();
        stepper.frame_step();
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
        // check that the component was replicated
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .expect("component missing"),
            &Component1(1.0)
        );

        // update component
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert(Component1(2.0));
        stepper.frame_step();
        stepper.frame_step();

        // check that the component was not updated
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .expect("component missing"),
            &Component1(1.0)
        );
    }

    #[test]
    fn test_component_remove() {
        let mut stepper = BevyStepper::default();

        // spawn an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn((Replicate::default(), Component1(1.0)))
            .id();
        stepper.frame_step();
        stepper.frame_step();
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .expect("component missing"),
            &Component1(1.0)
        );

        // remove component
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .remove::<Component1>();
        stepper.frame_step();
        stepper.frame_step();

        // check that the component was replicated
        assert!(stepper
            .client_app
            .world
            .entity(client_entity)
            .get::<Component1>()
            .is_none());
    }

    #[test]
    fn test_replication_target_add() {
        let mut stepper = BevyStepper::default();

        let server_entity = stepper.server_app.world.spawn(Replicate::default()).id();
        stepper.frame_step();

        // check that a DespawnTracker was added
        assert!(stepper
            .server_app
            .world
            .entity(server_entity)
            .get::<DespawnTracker>()
            .is_some());
        // check that a ReplicateCache was added
        assert_eq!(
            stepper
                .server_app
                .world
                .resource::<server::ConnectionManager>()
                .replicate_component_cache
                .get(&server_entity)
                .expect("ReplicateCache missing"),
            &ReplicateCache {
                replication_target: NetworkTarget::All,
                replication_group: ReplicationGroup::new_from_entity(),
                visibility_mode: VisibilityMode::All,
                replication_clients_cache: vec![],
            }
        );
    }

    /// Check that if we switch the visibility mode, the entity gets spawned
    /// to the clients that now have visibility
    #[test]
    fn test_change_visibility_mode_spawn() {
        let mut stepper = BevyStepper::default();

        let server_entity = stepper
            .server_app
            .world
            .spawn(Replicate {
                target: ReplicationTarget {
                    replication: NetworkTarget::None,
                    ..default()
                },
                ..default()
            })
            .id();
        stepper.frame_step();
        stepper.frame_step();

        // set visibility to interest management
        stepper.server_app.world.entity_mut(server_entity).insert((
            VisibilityMode::InterestManagement,
            ReplicationTarget {
                replication: NetworkTarget::All,
                ..default()
            },
        ));
        stepper
            .server_app
            .world
            .resource_mut::<VisibilityManager>()
            .gain_visibility(ClientId::Netcode(TEST_CLIENT_ID), server_entity);

        stepper.frame_step();
        stepper.frame_step();
        stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
    }
}
