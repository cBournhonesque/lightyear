//! The server spawns an entity per connected client to store metadata about them.
//!
//! This module contains components and systems to manage the metadata on client entities.
use crate::server::clients::systems::handle_controlled_by_remove;
use crate::server::replication::send::Lifetime;
use crate::shared::sets::{InternalReplicationSet, ServerMarker};
use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

/// List of entities under the control of a client
#[derive(Component, Default, Debug, Deref, DerefMut, PartialEq)]
pub struct ControlledEntities(pub(crate) EntityHashMap<Lifetime>);

impl ControlledEntities {
    /// Check if the entity is controlled by the client
    pub fn contains(&self, entity: &Entity) -> bool {
        self.0.contains_key(entity)
    }

    /// Get the list of entities controlled by the client
    pub fn entities(&self) -> Vec<Entity> {
        self.0.keys().copied().collect()
    }
}

pub(crate) struct ClientsMetadataPlugin;

mod systems {
    use super::*;
    use crate::prelude::server::ControlledBy;
    use crate::server::clients::ControlledEntities;
    use crate::server::connection::ConnectionManager;
    use crate::server::events::DisconnectEvent;
    use tracing::{debug, trace};

    // TODO: remove entity in ControlledEntities lists after the component gets updated
    //  (e.g. control goes from client 1 to client 2)
    //  need to detect what the previous ControlledBy was to compute the change
    //  i.e. add the previous ControlledBy to the replicate cache?

    /// If the [`ControlledBy`] component gets updated, update the [`ControlledEntities`] component
    /// on the Client Entity
    pub(super) fn handle_controlled_by_update(
        sender: Res<ConnectionManager>,
        query: Query<(Entity, &ControlledBy), Changed<ControlledBy>>,
        mut client_query: Query<&mut ControlledEntities>,
    ) {
        for (entity, controlled_by) in query.iter() {
            // TODO: avoid clone
            sender
                .connected_targets(controlled_by.target.clone())
                .for_each(|client_id| {
                    if let Ok(client_entity) = sender.client_entity(client_id) {
                        if let Ok(mut controlled_entities) = client_query.get_mut(client_entity) {
                            // first check if it already contains, to not trigger change detection needlessly
                            if controlled_entities.contains_key(&entity) {
                                return;
                            }
                            trace!(
                                "Adding entity {:?} to client {:?}'s controlled entities",
                                entity,
                                client_id,
                            );
                            controlled_entities.insert(entity, controlled_by.lifetime);
                        }
                    }
                });
        }
    }

    /// When the [`ControlledBy`] component gets removed from an entity, remove that entity from the list of
    /// [`ControlledEntities`] for the client
    pub(super) fn handle_controlled_by_remove(
        trigger: Trigger<OnRemove, ControlledBy>,
        query: Query<&ControlledBy>,
        mut client_query: Query<&mut ControlledEntities>,
        sender: Res<ConnectionManager>,
    ) {
        // OnRemove observers trigger before the actual removal
        let entity = trigger.entity();
        if let Ok(controlled_by) = query.get(entity) {
            // TODO: avoid clone
            sender
                .connected_targets(controlled_by.target.clone())
                .for_each(|client_id| {
                    if let Ok(client_entity) = sender.client_entity(client_id) {
                        if let Ok(mut controlled_entities) = client_query.get_mut(client_entity) {
                            // first check if it already contains, to not trigger change detection needlessly
                            if !controlled_entities.contains_key(&entity) {
                                return;
                            }
                            trace!(
                                "Removing entity {:?} to client {:?}'s controlled entities",
                                entity,
                                client_id,
                            );
                            controlled_entities.remove(&entity);
                        }
                    }
                })
        }
    }

    /// When a client disconnects, we despawn all the entities it controlled if the lifetime
    /// is SesssionBased
    pub(super) fn handle_client_disconnect(
        trigger: Trigger<DisconnectEvent>,
        mut commands: Commands,
        client_query: Query<&ControlledEntities>,
    ) {
        // TODO: should directly we use the client entity as the trigger entity?
        let client_entity = trigger.event().entity;
        let client_id = trigger.event().client_id;
        // despawn all the controlled entities for the disconnected client
        if let Ok(controlled_entities) = client_query.get(client_entity) {
            debug!(
                "Despawning all entities controlled by disconnected client {:?}",
                client_id
            );
            for (entity, lifetime) in controlled_entities.iter() {
                if lifetime == &Lifetime::SessionBased {
                    trace!(
                        "Despawning entity {entity:?} controlled by disconnected client {:?}",
                        client_id
                    );
                    if let Some(command) = commands.get_entity(*entity) {
                        command.despawn_recursive();
                    }
                }
            }
        }
        // despawn the client entity itself
        if let Some(command) = commands.get_entity(client_entity) {
            command.despawn_recursive();
        };
    }

    // TODO: is this necessary? calling server.stop() should already run the disconnection process
    //  for all clients
    // /// When the server gets disconnected, despawn the client entities.
    // pub(super) fn handle_server_disconnect(
    //     mut commands: Commands,
    //     client_query: Query<Entity, With<ControlledEntities>>,
    // ) {
    //     for client_entity in client_query.iter() {
    //         if let Some(command) = commands.get_entity(client_entity) {
    //             command.despawn_recursive();
    //         }
    //     }
    // }
}

impl Plugin for ClientsMetadataPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            systems::handle_controlled_by_update
                .in_set(InternalReplicationSet::<ServerMarker>::BeforeBuffer),
        );
        app.observe(handle_controlled_by_remove);
        // TODO: should we have a system that runs in the `Last` SystemSet instead? because the user might want to still have access
        //  to the client entity
        app.observe(systems::handle_client_disconnect);
        // we handle this in the `Last` `SystemSet` to let the user handle the disconnect event
        // however they want first, before the client entity gets despawned
        // app.add_systems(Last, systems::handle_server_disconnect);
    }
}

#[cfg(test)]
mod tests {
    use crate::client::networking::ClientCommands;
    use crate::prelude::server::{ConnectionManager, ControlledBy, Replicate};
    use crate::prelude::{client, ClientId, NetworkTarget, Replicated, ReplicationTarget};
    use crate::server::clients::ControlledEntities;
    use crate::server::replication::send::Lifetime;
    use crate::tests::multi_stepper::{MultiBevyStepper, TEST_CLIENT_ID_1, TEST_CLIENT_ID_2};
    use crate::tests::stepper::{BevyStepper, TEST_CLIENT_ID};
    use bevy::ecs::entity::EntityHashMap;
    use bevy::prelude::{default, Entity, With};

    /// Check that the Client Entities are updated after ControlledBy is added
    #[test]
    fn test_insert_controlled_by() {
        let mut stepper = MultiBevyStepper::default();

        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                controlled_by: ControlledBy {
                    target: NetworkTarget::Single(ClientId::Netcode(TEST_CLIENT_ID_1)),
                    ..default()
                },
                ..default()
            })
            .id();

        stepper.frame_step();

        // check that the entity was marked as controlled by client_1
        let client_entity_1 = stepper
            .server_app
            .world()
            .resource::<ConnectionManager>()
            .client_entity(ClientId::Netcode(TEST_CLIENT_ID_1))
            .unwrap();
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ControlledEntities>(client_entity_1)
                .unwrap(),
            &ControlledEntities(EntityHashMap::from_iter([(
                server_entity,
                Lifetime::SessionBased
            )]))
        );
        // check that the entity was not marked as controlled by client_2
        let client_entity_2 = stepper
            .server_app
            .world()
            .resource::<ConnectionManager>()
            .client_entity(ClientId::Netcode(TEST_CLIENT_ID_2))
            .unwrap();
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ControlledEntities>(client_entity_2)
                .unwrap(),
            &ControlledEntities(EntityHashMap::default())
        );
    }

    /// Check that the ControlledEntities components are updated after ControlledBy is removed
    #[test]
    fn test_removed_controlled_by() {
        let mut stepper = BevyStepper::default();

        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                controlled_by: ControlledBy {
                    target: NetworkTarget::All,
                    ..default()
                },
                ..default()
            })
            .id();

        stepper.frame_step();

        // check that the entity was marked as controlled by the client
        let client_entity = stepper
            .server_app
            .world()
            .resource::<ConnectionManager>()
            .client_entity(ClientId::Netcode(TEST_CLIENT_ID))
            .unwrap();
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ControlledEntities>(client_entity)
                .unwrap(),
            &ControlledEntities(EntityHashMap::from_iter([(
                server_entity,
                Lifetime::SessionBased
            )]))
        );

        // despawn the entity (which removes ControlledBy)
        stepper.server_app.world_mut().despawn(server_entity);

        // check that the ControlledBy Entities have been updated (via observer)
        assert!(!stepper
            .server_app
            .world()
            .get::<ControlledEntities>(client_entity)
            .unwrap()
            .contains_key(&server_entity));
    }

    /// Check that when a client disconnects, its controlled entities get despawned
    /// on the server
    #[test]
    fn test_controlled_by_despawn_on_client_disconnect() {
        let mut stepper = BevyStepper::default();

        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                controlled_by: ControlledBy {
                    target: NetworkTarget::All,
                    ..default()
                },
                ..default()
            })
            .id();
        let server_entity_2 = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                controlled_by: ControlledBy {
                    target: NetworkTarget::All,
                    lifetime: Lifetime::Persistent,
                },
                ..default()
            })
            .id();

        stepper.frame_step();

        // check that the entity was marked as controlled by client_1
        let client_entity = stepper
            .server_app
            .world()
            .resource::<ConnectionManager>()
            .client_entity(ClientId::Netcode(TEST_CLIENT_ID))
            .unwrap();
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ControlledEntities>(client_entity)
                .unwrap(),
            &ControlledEntities(EntityHashMap::from_iter([
                (server_entity, Lifetime::SessionBased),
                (server_entity_2, Lifetime::Persistent)
            ]))
        );

        // client disconnects
        stepper
            .client_app
            .world_mut()
            .commands()
            .disconnect_client();

        stepper.frame_step();
        assert!(stepper
            .server_app
            .world()
            .get_entity(server_entity)
            .is_none());
        assert!(stepper
            .server_app
            .world()
            .get_entity(server_entity_2)
            .is_some());
    }

    /// The owning client despawns the entity that they control.
    /// The server should receive the despawn. This will trigger the
    /// OnRemove<ControlledBy>, which should not panic
    /// See: https://github.com/cBournhonesque/lightyear/issues/546
    #[test]
    fn test_owning_client_despawns_entity() {
        let mut stepper = BevyStepper::default();
        let client_entity = stepper
            .client_app
            .world_mut()
            .spawn(client::Replicate::default())
            .id();
        // make sure the server replicated the entity
        for _ in 0..10 {
            stepper.frame_step();
        }
        let server_entity = stepper
            .server_app
            .world_mut()
            .query_filtered::<Entity, With<Replicated>>()
            .single(stepper.server_app.world());
        // add ControlledBy on the entity
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_entity)
            .insert(Replicate {
                target: ReplicationTarget {
                    target: NetworkTarget::None,
                },
                controlled_by: ControlledBy {
                    target: NetworkTarget::Single(ClientId::Netcode(TEST_CLIENT_ID)),
                    ..default()
                },
                ..default()
            });

        // despawn the entity on the client
        stepper.client_app.world_mut().despawn(client_entity);
        // as described in https://github.com/cBournhonesque/lightyear/issues/546,
        // when the observer is triggered the `ConnectionManager` is not available if we use
        // world.resource_scope during `receive`, so the function panics
        for _ in 0..10 {
            stepper.frame_step();
        }
    }
}
