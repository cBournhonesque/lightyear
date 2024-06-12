/*! Main visibility module, where you can immediately update the visibility of an entity for a given client

# Visibility

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
use crate::prelude::{is_started, ClientId};
use crate::shared::sets::{InternalReplicationSet, ServerMarker};
use bevy::ecs::entity::EntityHashSet;
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
    gained: HashMap<ClientId, EntityHashSet>,
    lost: HashMap<ClientId, EntityHashSet>,
}

/// Resource that manages the visibility of entities for clients
///
/// You can call the two functions
/// - [`gain_visibility`](VisibilityManager::gain_visibility)
/// - [`lose_visibility`](VisibilityManager::lose_visibility)
///
/// to update the visibility of an entity for a given client.
#[derive(Resource, Debug, Default)]
pub struct VisibilityManager {
    events: VisibilityEvents,
}

impl VisibilityManager {
    /// Gain visibility of an entity for a given client.
    ///
    /// The visibility status gets cached and will be maintained until is it changed.
    pub fn gain_visibility(&mut self, client: ClientId, entity: Entity) -> &mut Self {
        self.events.lost.entry(client).and_modify(|set| {
            set.remove(&entity);
        });
        self.events.gained.entry(client).or_default().insert(entity);
        self
    }

    /// Lost visibility of an entity for a given client
    pub fn lose_visibility(&mut self, client: ClientId, entity: Entity) -> &mut Self {
        self.events.gained.entry(client).and_modify(|set| {
            set.remove(&entity);
        });
        self.events.lost.entry(client).or_default().insert(entity);
        self
    }

    // NOTE: this might not be needed because we drain the event cache every Send update
    // /// Remove all visibility events for a given client when they disconnect
    // ///
    // /// Called to release the memory associated with the client
    // pub(crate) fn handle_client_disconnection(&mut self, client: ClientId) {
    //     self.events.gained.remove(&client);
    //     self.events.lost.remove(&client);
    // }
}

pub(super) mod systems {
    use super::*;

    use crate::prelude::VisibilityMode;

    use bevy::prelude::DetectChanges;

    // NOTE: this might not be needed because we drain the event cache every Send update
    // /// Clear the internal room buffers when a client disconnects
    // pub fn handle_client_disconnect(
    //     mut manager: ResMut<VisibilityManager>,
    //     mut disconnect_events: EventReader<DisconnectEvent>,
    // ) {
    //     for event in disconnect_events.read() {
    //         let client_id = event.context();
    //         manager.handle_client_disconnection(*client_id);
    //     }
    // }

    /// If VisibilityMode becomes InterestManagement, add ReplicateVisibility to the entity
    /// If VisibilityMode becomes All, remove ReplicateVisibility from the entity
    ///
    /// Run this before the visibility systems and the replication buffer systems
    /// so that the visibility cache can be updated before the replication systems
    pub(in crate::server::visibility) fn add_replicate_visibility(
        mut commands: Commands,
        query: Query<(Entity, Ref<VisibilityMode>, Option<&ReplicateVisibility>)>,
    ) {
        for (entity, visibility_mode, replicate_visibility) in query.iter() {
            if visibility_mode.is_changed() {
                match visibility_mode.as_ref() {
                    VisibilityMode::InterestManagement => {
                        // do not overwrite the visibility if it already exists
                        if replicate_visibility.is_none() {
                            debug!("Adding ReplicateVisibility component for entity {entity:?}");
                            commands
                                .entity(entity)
                                .insert(ReplicateVisibility::default());
                        }
                    }
                    VisibilityMode::All => {
                        commands.entity(entity).remove::<ReplicateVisibility>();
                    }
                }
            }
        }
    }

    /// System that updates the visibility cache of each Entity based on the visibility events.
    pub fn update_visibility_from_events(
        mut manager: ResMut<VisibilityManager>,
        mut visibility: Query<&mut ReplicateVisibility>,
    ) {
        if manager.events.gained.is_empty() && manager.events.lost.is_empty() {
            return;
        }
        trace!("Visibility events: {:?}", manager.events);
        for (client, mut entities) in manager.events.lost.drain() {
            entities.drain().for_each(|entity| {
                if let Ok(mut cache) = visibility.get_mut(entity) {
                    // Only lose visibility if the client was visible to the entity
                    // (to avoid multiple despawn messages)
                    if let Some(vis) = cache.clients_cache.get_mut(&client) {
                        trace!("lose visibility for entity {entity:?} and client {client:?}");
                        *vis = ClientVisibility::Lost;
                    }
                }
            });
        }
        for (client, mut entities) in manager.events.gained.drain() {
            entities.drain().for_each(|entity| {
                if let Ok(mut cache) = visibility.get_mut(entity) {
                    // if the entity was already visible (Visibility::Maintained), be careful to not set it to
                    // Visibility::Gained as it would trigger a spawn replication action
                    //
                    // we don't need to check if the entity was set to Lost in the same update,
                    // since calling gain_visibility removes the entity from the lost_visibility queue
                    cache
                        .clients_cache
                        .entry(client)
                        .or_insert(ClientVisibility::Gained);
                }
            });
        }
    }

    /// After replication, update the Replication Cache:
    /// - Visibility Gained becomes Visibility Maintained
    /// - Visibility Lost gets removed from the cache
    pub fn update_replicate_visibility(mut query: Query<(Entity, &mut ReplicateVisibility)>) {
        for (entity, mut replicate) in query.iter_mut() {
            replicate
                .clients_cache
                .retain(|client_id, visibility| match visibility {
                    ClientVisibility::Gained => {
                        trace!(
                            "Visibility for client {client_id:?} and entity {entity:?} goes from gained to maintained"
                        );
                        *visibility = ClientVisibility::Maintained;
                        true
                    }
                    ClientVisibility::Lost => {
                        trace!("remove client {client_id:?} and entity {entity:?} from visibility cache");
                        false
                    }
                    ClientVisibility::Maintained => true,
                });
            // error!("replicate.clients_cache: {0:?}", replicate.clients_cache);
        }
    }

    /// Whenever the visibility of an entity changes, update the replication metadata cache
    /// so that we can correctly replicate the despawn to the correct clients
    pub(super) fn update_replication_cache(
        mut sender: ResMut<ConnectionManager>,
        mut query: Query<(Entity, Ref<VisibilityMode>, Option<&ReplicateVisibility>)>,
    ) {
        for (entity, visibility_mode, replicate_visibility) in query.iter_mut() {
            match visibility_mode.as_ref() {
                VisibilityMode::InterestManagement => {
                    if visibility_mode.is_changed() {
                        if let Some(cache) = sender.replicate_component_cache.get_mut(&entity) {
                            cache.visibility_mode = VisibilityMode::InterestManagement;
                            if let Some(replicate_visibility) = replicate_visibility {
                                cache.replication_clients_cache = replicate_visibility
                                    .clients_cache
                                    .iter()
                                    .filter_map(|(client, visibility)| {
                                        if *visibility != ClientVisibility::Lost {
                                            Some(*client)
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();
                            } else {
                                cache.replication_clients_cache.clear();
                            }
                        }
                    }
                }
                VisibilityMode::All => {
                    if visibility_mode.is_changed() && !visibility_mode.is_added() {
                        if let Some(cache) = sender.replicate_component_cache.get_mut(&entity) {
                            cache.visibility_mode = VisibilityMode::All;
                            cache.replication_clients_cache.clear();
                        }
                    }
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

/// Plugin that handles the visibility system
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
                    InternalReplicationSet::<ServerMarker>::BeforeBuffer,
                    VisibilitySet::UpdateVisibility,
                    InternalReplicationSet::<ServerMarker>::Buffer,
                    VisibilitySet::VisibilityCleanup,
                )
                    .run_if(is_started)
                    .chain(),
                // the visibility systems can run every send_interval
                (
                    VisibilitySet::UpdateVisibility,
                    VisibilitySet::VisibilityCleanup,
                )
                    .in_set(InternalReplicationSet::<ServerMarker>::SendMessages),
            ),
        );
        // SYSTEMS
        // NOTE: this might not be needed because we drain the event cache every Send update
        // app.add_systems(
        //     PreUpdate,
        //     systems::handle_client_disconnect.after(InternalMainSet::<ServerMarker>::EmitEvents),
        // );
        app.add_systems(
            PostUpdate,
            (
                systems::add_replicate_visibility
                    .in_set(InternalReplicationSet::<ServerMarker>::BeforeBuffer),
                systems::update_visibility_from_events.in_set(VisibilitySet::UpdateVisibility),
                systems::update_replication_cache
                    .in_set(InternalReplicationSet::<ServerMarker>::AfterBuffer),
                systems::update_replicate_visibility.in_set(VisibilitySet::VisibilityCleanup),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Multiple entities gain visibility for a given client
    #[test]
    fn test_multiple_visibility_gain() {
        let mut app = App::new();
        app.world_mut().init_resource::<VisibilityManager>();
        let entity1 = app.world_mut().spawn(ReplicateVisibility::default()).id();
        let entity2 = app.world_mut().spawn(ReplicateVisibility::default()).id();
        let client = ClientId::Netcode(1);

        app.world_mut()
            .resource_mut::<VisibilityManager>()
            .gain_visibility(client, entity1);
        app.world_mut()
            .resource_mut::<VisibilityManager>()
            .gain_visibility(client, entity2);

        assert_eq!(
            app.world_mut()
                .resource_mut::<VisibilityManager>()
                .events
                .gained
                .len(),
            1
        );
        assert_eq!(
            app.world_mut()
                .resource_mut::<VisibilityManager>()
                .events
                .gained
                .get(&client)
                .unwrap()
                .len(),
            2
        );
        app.world_mut()
            .run_system_once(systems::update_visibility_from_events);
        assert_eq!(
            app.world_mut()
                .resource_mut::<VisibilityManager>()
                .events
                .gained
                .len(),
            0
        );
        assert_eq!(
            app.world()
                .entity(entity1)
                .get::<ReplicateVisibility>()
                .unwrap()
                .clients_cache
                .get(&client)
                .unwrap(),
            &ClientVisibility::Gained
        );
        assert_eq!(
            app.world()
                .entity(entity2)
                .get::<ReplicateVisibility>()
                .unwrap()
                .clients_cache
                .get(&client)
                .unwrap(),
            &ClientVisibility::Gained
        );

        // After we used the visibility events, check how they are updated for bookkeeping
        // - Lost -> removed from cache
        // - Gained -> Maintained
        app.world_mut()
            .resource_mut::<VisibilityManager>()
            .lose_visibility(client, entity1);
        app.world_mut()
            .run_system_once(systems::update_visibility_from_events);
        assert_eq!(
            app.world()
                .entity(entity1)
                .get::<ReplicateVisibility>()
                .unwrap()
                .clients_cache
                .get(&client)
                .unwrap(),
            &ClientVisibility::Lost
        );
        assert_eq!(
            app.world()
                .entity(entity2)
                .get::<ReplicateVisibility>()
                .unwrap()
                .clients_cache
                .get(&client)
                .unwrap(),
            &ClientVisibility::Gained
        );
        app.world_mut()
            .run_system_once(systems::update_replicate_visibility);
        assert!(app
            .world()
            .entity(entity1)
            .get::<ReplicateVisibility>()
            .unwrap()
            .clients_cache
            .is_empty());
        assert_eq!(
            app.world()
                .entity(entity2)
                .get::<ReplicateVisibility>()
                .unwrap()
                .clients_cache
                .get(&client)
                .unwrap(),
            &ClientVisibility::Maintained
        );
    }
}
