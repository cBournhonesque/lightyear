//! The server spawns an entity per connected client to store metadata about them.
//!
//! This module contains components and systems to manage the metadata on client entities.
use crate::prelude::ClientId;
use crate::shared::sets::{InternalReplicationSet, ServerMarker};
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

/// List of entities under the control of a client
#[derive(Component, Default, Debug, Deref, DerefMut)]
pub struct ControlledEntities(pub EntityHashSet);

pub(crate) struct ClientsMetadataPlugin;

mod systems {
    use super::*;
    use crate::prelude::Replicate;
    use crate::server::clients::ControlledEntities;
    use crate::server::connection::ConnectionManager;
    use crate::server::events::DisconnectEvent;
    use crate::shared::replication::components::ControlledBy;
    use crate::shared::replication::network_target::NetworkTarget;
    use tracing::{debug, error, trace};

    // TODO: remove entity from ControlledBy when ControlledBy gets removed! (via observers)?
    // TODO: remove entity in controlled by lists after the component gets updated

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
                    client_id
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
                error!(
                    "Despawning all entities controlled by client {:?}",
                    event.client_id
                );
                for entity in controlled_entities.iter() {
                    error!(
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
