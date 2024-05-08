/*! # Visibility

This module provides a [`VisibilityManager`] resource that allows you to update the visibility of entities in an immediate fashion.

Visibilities are cached, so after you set an entity to `visible` for a client, it will remain visible
until you change the setting again.

```rust
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear::prelude::server::*;

fn my_system(
    mut visibility_manager: ResMut<VisibilityManager>,
) {
    // you can update the visibility like so
    visibility_manager.gain_visibility(ClientId::Netcode(1), Entity::PLACEHOLDER);
    visibility_manager.lose_visibility(ClientId::Netcode(2), Entity::PLACEHOLDER);
}
```
*/
use crate::prelude::server::ConnectionManager;
use crate::prelude::ClientId;
use crate::server::networking::is_started;
use crate::server::visibility::room::{RoomManager, RoomSystemSets};
use crate::shared::sets::{InternalMainSet, InternalReplicationSet, ServerMarker};
use bevy::prelude::*;
use bevy::utils::HashMap;
use tracing::trace;

/// Event related to [`Entities`](Entity) which are visible to a client
#[derive(Debug, PartialEq, Clone, Copy, Reflect)]
pub(crate) enum ClientVisibility {
    /// the entity was not replicated to the client, but now is
    Gained,
    /// the entity was replicated to the client, but not anymore
    Lost,
    /// the entity was already replicated to the client, and still is
    Maintained,
}

#[derive(Component, Clone, Default, PartialEq, Debug, Reflect)]
pub(crate) struct ReplicateVisibility {
    /// List of clients that the entity is currently replicated to.
    /// Will be updated before the other replication systems
    pub(crate) clients_cache: HashMap<ClientId, ClientVisibility>,
}

#[derive(Debug, Default)]
struct VisibilityEvents {
    gained: HashMap<ClientId, Entity>,
    lost: HashMap<ClientId, Entity>,
}

#[derive(Resource, Debug, Default)]
pub struct VisibilityManager {
    events: VisibilityEvents,
}

impl VisibilityManager {
    pub fn gain_visibility(&mut self, client: ClientId, entity: Entity) {
        self.events.gained.insert(client, entity);
    }

    pub fn lose_visibility(&mut self, client: ClientId, entity: Entity) {
        self.events.lost.insert(client, entity);
    }
}

pub(super) mod systems {
    use super::*;
    use crate::shared::replication::ReplicationSend;
    use bevy::prelude::DetectChanges;

    /// System that updates the visibility cache of each Entity based on the visibility events.
    pub fn update_visibility_from_events(
        mut visibility_events: ResMut<VisibilityManager>,
        mut visibility: Query<&mut ReplicateVisibility>,
    ) {
        if visibility_events.events.gained.is_empty() && visibility_events.events.lost.is_empty() {
            return;
        }
        // NOTE: we handle lost events before gained events so that if one event in the queue
        //  removes the visibility, and another one gains it, the visibility is maintained
        for (client, entity) in visibility_events.events.lost.drain() {
            if let Ok(mut cache) = visibility.get_mut(entity) {
                error!("Want to lose visibility for entity {entity:?} and client {client:?}. Cache: {cache:?}");
                if let Some(vis) = cache.clients_cache.get_mut(&client) {
                    error!("lose visibility for entity {entity:?} and client {client:?}");
                    *vis = ClientVisibility::Lost;
                }
            }
        }
        for (client, entity) in visibility_events.events.gained.drain() {
            if let Ok(mut cache) = visibility.get_mut(entity) {
                cache
                    .clients_cache
                    .entry(client)
                    .and_modify(|vis| {
                        // if the visibility was lost above, then that means that the entity was visible
                        // for this client, so we just maintain it instead
                        if *vis == ClientVisibility::Lost {
                            error!("visibility for entity {entity:?} and client {client:?} goes from lost to maintained");
                            *vis = ClientVisibility::Maintained;
                        }
                    })
                    // if the entity was not visible, the visibility is gained
                    .or_insert(ClientVisibility::Gained);
            }
        }
    }

    /// After replication, update the Replication Cache:
    /// - Visibility Gained becomes Visibility Maintained
    /// - Visibility Lost gets removed from the cache
    pub fn update_replicate_visibility(mut query: Query<&mut ReplicateVisibility>) {
        for mut replicate in query.iter_mut() {
            replicate
                .clients_cache
                .retain(|client_id, visibility| match visibility {
                    ClientVisibility::Gained => {
                        error!(
                            "Visibility for client {client_id:?} goes from gained to maintained"
                        );
                        *visibility = ClientVisibility::Maintained;
                        true
                    }
                    ClientVisibility::Lost => {
                        error!("remove client {client_id:?} from room cache");
                        false
                    }
                    ClientVisibility::Maintained => true,
                });
        }
    }

    /// Whenever the visibility of an entity changes, update the despawn metadata cache
    /// so that we can correctly replicate the despawn to the correct clients
    pub fn update_despawn_metadata_cache(
        mut connection_manager: ResMut<ConnectionManager>,
        mut query: Query<(Entity, &mut ReplicateVisibility)>,
    ) {
        for (entity, visibility) in query.iter_mut() {
            if visibility.is_changed() {
                if let Some(despawn_metadata) = connection_manager
                    .get_mut_replicate_despawn_cache()
                    .get_mut(&entity)
                {
                    let new_cache = visibility.clients_cache.keys().copied().collect();
                    despawn_metadata.replication_clients_cache = new_cache;
                }
            }
        }
    }
}

/// System sets related to Rooms
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum VisibilitySet {
    /// Update the visibility cache based on the visibility events
    UpdateVisibility,
    /// Perform bookkeeping for the visibility caches
    VisibilityCleanup,
}

#[derive(Default)]
pub(crate) struct VisibilityPlugin;

impl Plugin for VisibilityPlugin {
    fn build(&self, app: &mut App) {
        // RESOURCES
        app.init_resource::<VisibilityManager>();
        // SETS
        app.configure_sets(
            PostUpdate,
            (
                (
                    // update replication caches must happen before replication, but after we add ReplicateVisibility
                    InternalReplicationSet::<ServerMarker>::HandleReplicateUpdate,
                    VisibilitySet::UpdateVisibility,
                    InternalReplicationSet::<ServerMarker>::Buffer,
                    VisibilitySet::VisibilityCleanup,
                )
                    .run_if(is_started)
                    .chain(),
                // the room systems can run every send_interval
                (
                    VisibilitySet::UpdateVisibility,
                    VisibilitySet::VisibilityCleanup,
                )
                    .in_set(InternalMainSet::<ServerMarker>::Send),
            ),
        );
        // SYSTEMS
        app.add_systems(
            PostUpdate,
            (
                (
                    systems::update_visibility_from_events,
                    systems::update_despawn_metadata_cache,
                )
                    .chain()
                    .in_set(VisibilitySet::UpdateVisibility),
                systems::update_replicate_visibility.in_set(VisibilitySet::VisibilityCleanup),
            ),
        );
    }
}
