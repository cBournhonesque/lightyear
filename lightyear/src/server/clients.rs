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
pub struct ControlledEntities(pub EntityHashMap<Lifetime>);

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
        mut commands: Commands,
        client_query: Query<&ControlledEntities>,
        mut events: EventReader<DisconnectEvent>,
    ) {
        for event in events.read() {
            // despawn all the controlled entities for the disconnected client
            if let Ok(controlled_entities) = client_query.get(event.entity) {
                debug!(
                    "Despawning all entities controlled by disconnected client {:?}",
                    event.client_id
                );
                for (entity, lifetime) in controlled_entities.iter() {
                    if lifetime == &Lifetime::SessionBased {
                        trace!(
                            "Despawning entity {entity:?} controlled by disconnected client {:?}",
                            event.client_id
                        );
                        if let Some(command) = commands.get_entity(*entity) {
                            command.despawn_recursive();
                        }
                    }
                }
            }
            // despawn the entity itself
            if let Some(command) = commands.get_entity(event.entity) {
                command.despawn_recursive();
            };
        }
    }
}

impl Plugin for ClientsMetadataPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            systems::handle_controlled_by_update
                .in_set(InternalReplicationSet::<ServerMarker>::BeforeBuffer),
        );
        app.observe(handle_controlled_by_remove);
        // we handle this in the `Last` `SystemSet` to let the user handle the disconnect event
        // however they want first, before the client entity gets despawned
        app.add_systems(Last, systems::handle_client_disconnect);
    }
}

#[cfg(test)]
mod tests {
    use crate::client::networking::ClientCommands;
    use crate::prelude::server::{ConnectionManager, ControlledBy, Replicate};
    use crate::prelude::{ClientId, NetworkTarget};
    use crate::server::clients::ControlledEntities;
    use crate::server::replication::send::Lifetime;
    use crate::tests::multi_stepper::{MultiBevyStepper, TEST_CLIENT_ID_1, TEST_CLIENT_ID_2};
    use crate::tests::stepper::{BevyStepper, Step, TEST_CLIENT_ID};
    use bevy::ecs::entity::EntityHashMap;
    use bevy::prelude::default;

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
}
