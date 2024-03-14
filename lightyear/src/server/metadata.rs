//! Module that handles the creation of a single Entity per client
//! These can be used to:
//! - track some information about each client
//! - send information to clients about themselves (such as their global ClientId, independent from the connection's ClientId) or about other clients
use crate::prelude::{ClientId, MainSet, NetworkTarget, Protocol};
use crate::server::events::{ConnectEvent, DisconnectEvent};
use crate::shared::replication::components::Replicate;
use crate::shared::replication::metadata::ClientMetadata;
use bevy::prelude::*;

pub(crate) struct ClientMetadataPlugin<P: Protocol> {
    marker: std::marker::PhantomData<P>,
}

impl<P: Protocol> Default for ClientMetadataPlugin<P> {
    fn default() -> Self {
        Self {
            marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol> Plugin for ClientMetadataPlugin<P> {
    fn build(&self, app: &mut App) {
        app
            // RESOURCE
            .init_resource::<GlobalMetadata>()
            // SYSTEM
            .add_systems(
                PreUpdate,
                (spawn_client_entity::<P>, despawn_client_entity)
                    .after(MainSet::Receive)
                    .before(MainSet::ReceiveFlush),
            );
    }
}

type EntityHashMap<K, V> = hashbrown::HashMap<K, V, bevy::ecs::entity::EntityHash>;

#[derive(Debug, Default, Resource)]
pub struct GlobalMetadata {
    /// map from client_id to the entity that holds metadata about the client
    pub client_id_to_entity: EntityHashMap<ClientId, Entity>,
}

/// Whenever we receive a new client connection, spawn a new entity for it that holds some metadata about it
fn spawn_client_entity<P: Protocol>(
    mut commands: Commands,
    mut global_metadata: ResMut<GlobalMetadata>,
    mut event: EventReader<ConnectEvent>,
) {
    for event in event.read() {
        let client_id = *event.context();
        let client_entity = commands.spawn((
            ClientMetadata { client_id },
            // for now, only replicate to the client itself
            Replicate::<P> {
                replication_target: NetworkTarget::Single(client_id),
                ..default()
            },
        ));
        global_metadata
            .client_id_to_entity
            .insert(client_id, client_entity.id());
    }
}

/// When a client disconnects, despawn the entity that holds metadata about it
/// (and update the global client metadata resource)
fn despawn_client_entity(
    mut commands: Commands,
    mut global_metadata: ResMut<GlobalMetadata>,
    mut event: EventReader<DisconnectEvent>,
) {
    for event in event.read() {
        let client_id = event.context();
        if let Some(entity) = global_metadata.client_id_to_entity.remove(client_id) {
            commands.entity(entity).despawn();
        }
    }
}
