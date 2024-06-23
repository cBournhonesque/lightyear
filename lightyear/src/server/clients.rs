//! The server spawns an entity per connected client to store metadata about them.
//!
//! This module contains components and systems to manage the metadata on client entities.
use crate::prelude::ClientId;
use crate::shared::sets::{InternalReplicationSet, ServerMarker};
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

/// List of entities under the control of a client
#[derive(Component, Default, Debug, Deref, DerefMut, PartialEq)]
pub struct ControlledEntities(pub EntityHashSet);

pub(crate) struct ClientsMetadataPlugin;

mod systems {
    use super::*;
    use crate::prelude::server::ControlledBy;
    use crate::server::clients::ControlledEntities;
    use crate::server::connection::ConnectionManager;
    use crate::server::events::DisconnectEvent;
    use crate::shared::replication::network_target::NetworkTarget;
    use tracing::{debug, trace};

    // TODO: remove entity from ControlledBy when ControlledBy gets removed! (via observers)?
    //  but does the OnRemove component get called when the entity gets despawned?

    // TODO: remove entity in controlledby lists after the component gets updated
    //  need to detect what the previous ControlledBy was to compute the change

    /// If the ControlledBy component gets update, update the ControlledEntities component
    /// on the Client Entity
    pub(super) fn handle_controlled_by_update(
        sender: Res<ConnectionManager>,
        query: Query<(Entity, &ControlledBy), Changed<ControlledBy>>,
        mut client_query: Query<&mut ControlledEntities>,
    ) {
        let update_controlled_entities =
            |entity: Entity,
             client_id: ClientId,
             client_query: &mut Query<&mut ControlledEntities>,
             sender: &ConnectionManager| {
                trace!(
                    "Adding entity {:?} to client {:?}'s controlled entities",
                    entity,
                    client_id,
                );
                if let Ok(client_entity) = sender.client_entity(client_id) {
                    if let Ok(mut controlled_entities) = client_query.get_mut(client_entity) {
                        // first check if it already contains, to not trigger change detection needlessly
                        if controlled_entities.contains(&entity) {
                            return;
                        }
                        controlled_entities.insert(entity);
                    }
                }
            };

        for (entity, controlled_by) in query.iter() {
            match &controlled_by.target {
                NetworkTarget::None => {}
                NetworkTarget::Single(client_id) => {
                    update_controlled_entities(entity, *client_id, &mut client_query, &sender);
                }
                NetworkTarget::Only(client_ids) => client_ids.iter().for_each(|client_id| {
                    update_controlled_entities(entity, *client_id, &mut client_query, &sender);
                }),
                _ => {
                    let client_ids: Vec<ClientId> = sender.connected_clients().collect();
                    client_ids.iter().for_each(|client_id| {
                        update_controlled_entities(entity, *client_id, &mut client_query, &sender);
                    });
                }
            }
        }
    }

    /// When a client disconnect, we despawn all the entities it controlled
    pub(super) fn handle_client_disconnect(
        mut commands: Commands,
        client_query: Query<&ControlledEntities>,
        mut events: EventReader<DisconnectEvent>,
    ) {
        for event in events.read() {
            // despawn all the controlled entities for the disconnected client
            if let Ok(controlled_entities) = client_query.get(event.entity) {
                debug!(
                    "Despawning all entities controlled by client {:?}",
                    event.client_id
                );
                for entity in controlled_entities.iter() {
                    debug!(
                        "Despawning entity {entity:?} controlled by client {:?}",
                        event.client_id
                    );
                    commands.entity(*entity).despawn_recursive();
                }
            }
            // despawn the entity itself
            commands.entity(event.entity).despawn_recursive();
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
    use crate::tests::multi_stepper::{MultiBevyStepper, TEST_CLIENT_ID_1, TEST_CLIENT_ID_2};
    use crate::tests::stepper::{BevyStepper, Step, TEST_CLIENT_ID};
    use bevy::ecs::entity::EntityHashSet;
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
            &ControlledEntities(EntityHashSet::from_iter([server_entity]))
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
            &ControlledEntities(EntityHashSet::default())
        );
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
            &ControlledEntities(EntityHashSet::from_iter([server_entity]))
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
    }
}
