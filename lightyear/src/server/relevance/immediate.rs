/*! Main network relevance module, where you can immediately update the network relevance of an entity for a given client

# Network Relevance

The **network relevance** is used to determine which entities are replicated to a client. The server will only replicate the entities that are relevant to a client. If the client stops being relevant, the server will despawn that entity for that client. This lets you save bandwidth by only sending the necessary data to each client.

This module provides a [`RelevanceManager`] resource that allows you to update the relevance of entities in an immediate fashion.

Network Relevance are cached, so after you set an entity to `relevant` for a client, it will remain relevant
until you change the setting again.

```rust
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear::prelude::server::*;

fn my_system(
    mut relevance_manager: ResMut<RelevanceManager>,
) {
    // you can update the relevance like so
    relevance_manager.gain_relevance(ClientId::Netcode(1), Entity::PLACEHOLDER);
    relevance_manager.lose_relevance(ClientId::Netcode(2), Entity::PLACEHOLDER);
}
```
*/
use crate::prelude::{server::is_started, ClientId};
use crate::shared::sets::{InternalReplicationSet, ServerMarker};
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use bevy::utils::HashMap;
use tracing::trace;

/// Event related to [`Entities`](Entity) which are relevant to a client
#[derive(Debug, PartialEq, Clone, Copy, Reflect)]
pub(crate) enum ClientRelevance {
    /// the entity was not replicated to the client, but now is
    Gained,
    /// the entity was replicated to the client, but not anymore
    Lost,
    /// the entity was already replicated to the client, and still is
    Maintained,
}

#[derive(Component, Clone, Default, PartialEq, Debug, Reflect)]
#[reflect(Component)]
pub(crate) struct CachedNetworkRelevance {
    /// List of clients that the entity is currently replicated to.
    /// Will be updated before the other replication systems
    pub(crate) clients_cache: HashMap<ClientId, ClientRelevance>,
}

#[derive(Debug, Default)]
struct RelevanceEvents {
    gained: HashMap<ClientId, EntityHashSet>,
    lost: HashMap<ClientId, EntityHashSet>,
}

/// Resource that manages the network relevance of entities for clients
///
/// You can call the two functions
/// - [`gain_relevance`](RelevanceManager::gain_relevance)
/// - [`lose_relevance`](RelevanceManager::lose_relevance)
///
/// to update the relevance of an entity for a given client.
#[derive(Resource, Debug, Default)]
pub struct RelevanceManager {
    events: RelevanceEvents,
}

impl RelevanceManager {
    /// Gain relevance of an entity for a given client.
    ///
    /// The relevance status gets cached and will be maintained until is it changed.
    pub fn gain_relevance(&mut self, client: ClientId, entity: Entity) -> &mut Self {
        self.events.lost.entry(client).and_modify(|set| {
            set.remove(&entity);
        });
        self.events.gained.entry(client).or_default().insert(entity);
        self
    }

    /// Lost relevance of an entity for a given client
    pub fn lose_relevance(&mut self, client: ClientId, entity: Entity) -> &mut Self {
        self.events.gained.entry(client).and_modify(|set| {
            set.remove(&entity);
        });
        self.events.lost.entry(client).or_default().insert(entity);
        self
    }

    // NOTE: this might not be needed because we drain the event cache every Send update
    // /// Remove all relevance events for a given client when they disconnect
    // ///
    // /// Called to release the memory associated with the client
    // pub(crate) fn handle_client_disconnection(&mut self, client: ClientId) {
    //     self.events.gained.remove(&client);
    //     self.events.lost.remove(&client);
    // }
}

pub(super) mod systems {
    use super::*;

    use crate::prelude::NetworkRelevanceMode;

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

    /// If VisibilityMode becomes InterestManagement, add CachedNetworkRelevance to the entity
    /// If VisibilityMode becomes All, remove CachedNetworkRelevance from the entity
    ///
    /// Run this before the relevance systems and the replication buffer systems
    /// so that the relevance cache can be updated before the replication systems
    pub(in crate::server::relevance) fn add_cached_network_relevance(
        mut commands: Commands,
        query: Query<(
            Entity,
            Ref<NetworkRelevanceMode>,
            Option<&CachedNetworkRelevance>,
        )>,
    ) {
        for (entity, relevance_mode, cached_relevance) in query.iter() {
            if relevance_mode.is_changed() {
                match relevance_mode.as_ref() {
                    NetworkRelevanceMode::InterestManagement => {
                        // do not overwrite the relevance if it already exists
                        if cached_relevance.is_none() {
                            debug!("Adding CachedNetworkRelevance component for entity {entity:?}");
                            commands
                                .entity(entity)
                                .insert(CachedNetworkRelevance::default());
                        }
                    }
                    NetworkRelevanceMode::All => {
                        commands.entity(entity).remove::<CachedNetworkRelevance>();
                    }
                }
            }
        }
    }

    /// System that updates the relevance cache of each Entity based on the relevance events.
    pub fn update_relevance_from_events(
        mut manager: ResMut<RelevanceManager>,
        mut relevance: Query<&mut CachedNetworkRelevance>,
    ) {
        if manager.events.gained.is_empty() && manager.events.lost.is_empty() {
            return;
        }
        trace!("Relevance events: {:?}", manager.events);
        for (client, mut entities) in manager.events.lost.drain() {
            entities.drain().for_each(|entity| {
                if let Ok(mut cache) = relevance.get_mut(entity) {
                    // Only lose relevance if the client was visible to the entity
                    // (to avoid multiple despawn messages)
                    if let Some(vis) = cache.clients_cache.get_mut(&client) {
                        trace!("lose relevance for entity {entity:?} and client {client:?}");
                        *vis = ClientRelevance::Lost;
                    }
                }
            });
        }
        for (client, mut entities) in manager.events.gained.drain() {
            entities.drain().for_each(|entity| {
                if let Ok(mut cache) = relevance.get_mut(entity) {
                    // if the entity was already relevant (Relevance::Maintained), be careful to not set it to
                    // Relevance::Gained as it would trigger a spawn replication action
                    //
                    // we don't need to check if the entity was set to Lost in the same update,
                    // since calling gain_relevance removes the entity from the lost_relevance queue
                    cache
                        .clients_cache
                        .entry(client)
                        .or_insert(ClientRelevance::Gained);
                }
            });
        }
    }

    /// After replication, update the Replication Cache:
    /// - Relevance Gained becomes Relevance Maintained
    /// - Relevance Lost gets removed from the cache
    pub fn update_cached_relevance(mut query: Query<(Entity, &mut CachedNetworkRelevance)>) {
        for (entity, mut replicate) in query.iter_mut() {
            replicate
                .clients_cache
                .retain(|client_id, relevance| match relevance {
                    ClientRelevance::Gained => {
                        trace!(
                            "Relevance for client {client_id:?} and entity {entity:?} goes from gained to maintained"
                        );
                        *relevance = ClientRelevance::Maintained;
                        true
                    }
                    ClientRelevance::Lost => {
                        trace!("remove client {client_id:?} and entity {entity:?} from relevance cache");
                        false
                    }
                    ClientRelevance::Maintained => true,
                });
            // error!("replicate.clients_cache: {0:?}", replicate.clients_cache);
        }
    }
}

/// System sets related to Network Relevance
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum NetworkRelevanceSet {
    /// Update the relevance cache based on the relevance events
    UpdateRelevance,
    /// Perform bookkeeping for the relevance caches
    RelevanceCleanup,
}

/// Plugin that handles the relevance system
#[derive(Default)]
pub(crate) struct NetworkRelevancePlugin;

impl Plugin for NetworkRelevancePlugin {
    fn build(&self, app: &mut App) {
        // RESOURCES
        app.init_resource::<RelevanceManager>();
        // SETS
        app.configure_sets(
            PostUpdate,
            (
                (
                    // update replication caches must happen before replication, but after we add CachedNetworkRelevance
                    InternalReplicationSet::<ServerMarker>::BeforeBuffer,
                    NetworkRelevanceSet::UpdateRelevance,
                    InternalReplicationSet::<ServerMarker>::Buffer,
                    NetworkRelevanceSet::RelevanceCleanup,
                )
                    .run_if(is_started)
                    .chain(),
                // the relevance systems can run every send_interval
                (
                    NetworkRelevanceSet::UpdateRelevance,
                    NetworkRelevanceSet::RelevanceCleanup,
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
                systems::add_cached_network_relevance
                    .in_set(InternalReplicationSet::<ServerMarker>::BeforeBuffer),
                systems::update_relevance_from_events.in_set(NetworkRelevanceSet::UpdateRelevance),
                systems::update_cached_relevance.in_set(NetworkRelevanceSet::RelevanceCleanup),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Multiple entities gain relevance for a given client
    #[test]
    fn test_multiple_relevance_gain() {
        let mut app = App::new();
        app.world_mut().init_resource::<RelevanceManager>();
        let entity1 = app
            .world_mut()
            .spawn(CachedNetworkRelevance::default())
            .id();
        let entity2 = app
            .world_mut()
            .spawn(CachedNetworkRelevance::default())
            .id();
        let client = ClientId::Netcode(1);

        app.world_mut()
            .resource_mut::<RelevanceManager>()
            .gain_relevance(client, entity1);
        app.world_mut()
            .resource_mut::<RelevanceManager>()
            .gain_relevance(client, entity2);

        assert_eq!(
            app.world()
                .resource::<RelevanceManager>()
                .events
                .gained
                .len(),
            1
        );
        assert_eq!(
            app.world()
                .resource::<RelevanceManager>()
                .events
                .gained
                .get(&client)
                .unwrap()
                .len(),
            2
        );
        app.world_mut()
            .run_system_once(systems::update_relevance_from_events);
        assert_eq!(
            app.world()
                .resource::<RelevanceManager>()
                .events
                .gained
                .len(),
            0
        );
        assert_eq!(
            app.world()
                .entity(entity1)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache
                .get(&client)
                .unwrap(),
            &ClientRelevance::Gained
        );
        assert_eq!(
            app.world()
                .entity(entity2)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache
                .get(&client)
                .unwrap(),
            &ClientRelevance::Gained
        );

        // After we used the relevance events, check how they are updated for bookkeeping
        // - Lost -> removed from cache
        // - Gained -> Maintained
        app.world_mut()
            .resource_mut::<RelevanceManager>()
            .lose_relevance(client, entity1);
        app.world_mut()
            .run_system_once(systems::update_relevance_from_events);
        assert_eq!(
            app.world()
                .entity(entity1)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache
                .get(&client)
                .unwrap(),
            &ClientRelevance::Lost
        );
        assert_eq!(
            app.world()
                .entity(entity2)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache
                .get(&client)
                .unwrap(),
            &ClientRelevance::Gained
        );
        app.world_mut()
            .run_system_once(systems::update_cached_relevance);
        assert!(app
            .world()
            .entity(entity1)
            .get::<CachedNetworkRelevance>()
            .unwrap()
            .clients_cache
            .is_empty());
        assert_eq!(
            app.world()
                .entity(entity2)
                .get::<CachedNetworkRelevance>()
                .unwrap()
                .clients_cache
                .get(&client)
                .unwrap(),
            &ClientRelevance::Maintained
        );
    }
}
