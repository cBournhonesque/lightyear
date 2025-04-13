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
    relevance_manager.gain_relevance(PeerId::Netcode(1), Entity::PLACEHOLDER);
    relevance_manager.lose_relevance(PeerId::Netcode(2), Entity::PLACEHOLDER);
}
```
*/

use crate::send::ReplicationBufferSet;
use bevy::ecs::entity::hash_set::EntityHashSet;
use bevy::ecs::entity::EntityIndexSet;
use bevy::platform_support::collections::{HashMap, HashSet};
use bevy::prelude::*;
use lightyear_connection::prelude::PeerId;
use tracing::*;

/// Event related to [`Entities`](Entity) which are relevant to a client
#[derive(Debug, PartialEq, Clone, Copy, Reflect)]
pub(crate) enum VisibilityState {
    /// the entity was not replicated to the client, but now is
    Gained,
    /// the entity was replicated to the client, but not anymore
    Lost,
    /// the entity was already replicated to the client, and still is
    Maintained,
}

/// We will replicate this entity to the clients specified in the `Replicate` component.
/// On top of that, we will apply interest management logic to determine which peers should receive the entity
///
/// You can use [`gain_relevance`](crate::prelude::server::RelevanceManager::gain_relevance) and [`lose_relevance`](crate::prelude::server::RelevanceManager::lose_relevance)
/// to control the network relevance of entities.
///
/// You can also use the [`RoomManager`](crate::prelude::server::RoomManager) if you want to use rooms to control network relevance.
///
/// (the client still needs to be included in the [`Replicate`], the room is simply an additional constraint)
#[derive(Component, Clone, Default, PartialEq, Debug, Reflect)]
#[reflect(Component)]
pub struct NetworkVisibility {
    /// List of clients that the entity is currently replicated to.
    /// Will be updated before the other replication systems
    // pub(crate) clients_cache: HashMap<PeerId, VisibilityState>,
    pub(crate) gained: EntityHashSet,
    pub(crate) visible: EntityHashSet,
    pub(crate) lost: EntityHashSet,
}

impl NetworkVisibility {

    pub(crate) fn is_visible(
        &self, sender: Entity,
    ) -> bool {
        self.visible.contains(&sender) || self.gained.contains(&sender)
    }
    pub fn gain_visibility(
        &mut self,
        sender: Entity,
    ) {
        // if the entity was already relevant (Relevance::Maintained), be careful to not set it to
        // Relevance::Gained as it would trigger a duplicate spawn replication action
        if !self.visible.contains(&sender) {
            self.gained.insert(sender);
        }
        self.lost.remove(&sender);
    }

    pub fn lose_visibility(
        &mut self,
        sender: Entity,
    ) {
        // Only lose relevance if the client was visible to the entity
        // (to avoid multiple despawn messages)
        if self.gained.remove(&sender) {
            return;
        }
        if self.visible.remove(&sender) {
            self.lost.insert(sender);
        }
    }
}


/// Plugin that handles the visibility system
#[derive(Default)]
pub struct NetworkVisibilityPlugin;

impl NetworkVisibilityPlugin {

    /// Update the visibility for each replicated entity.
    /// Gained becomes Maintained, Lost becomes cleared.
    fn update_network_visibility(
        mut query: Query<&mut NetworkVisibility>
    ) {
        query.iter_mut().for_each(|mut vis| {
            // enable split borrows
            let mut vis = vis.as_mut();
            vis.lost.clear();
            vis.gained.drain().for_each(|peer| {
                vis.visible.insert(peer);
            });
        })
    }

}

impl Plugin for NetworkVisibilityPlugin {
    fn build(&self, app: &mut App) {
        // REFLECT
        app.register_type::<NetworkVisibility>();
        // SYSTEMS
        app.add_systems(
            PostUpdate,
            (
                Self::update_network_visibility.in_set(ReplicationBufferSet::AfterBuffer),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_visibility() {
        let mut app = App::new();
        app.add_plugins(NetworkVisibilityPlugin);
        let entity = app
            .world_mut()
            .spawn(NetworkVisibility::default())
            .id();

        let sender = app.world_mut().spawn_empty().id();

        app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().gain_visibility(sender);
        assert!(app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().gained.contains(&sender));

        // after an update: Gained -> Visible
        app.update();
        assert!(!app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().gained.contains(&sender));
        assert!(app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().visible.contains(&sender));

        // if an entity is already visible, we do not make it Gained
        app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().gain_visibility(sender);
        assert!(!app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().gained.contains(&sender));

        // entity now loses Visibility
        app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().lose_visibility(sender);
        assert!(!app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().visible.contains(&sender));
        assert!(app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().lost.contains(&sender));

        // after an update: Lost -> Cleared
        app.update();
        assert!(!app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().lost.contains(&sender));

        // if we Gain/Lose visibility in the same tick, do nothing
        app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().gain_visibility(sender);
        app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().lose_visibility(sender);
        assert!(!app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().gained.contains(&sender));
        assert!(!app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().lost.contains(&sender));
    }

    /// https://github.com/cBournhonesque/lightyear/issues/637
    /// Make sure that entity despawns aren't replicated to clients that don't have visibility of the entity
    /// E1 gains relevance with C1, E1 is replicated
    /// E1 loses relevance with C1, E1 is despawned
    /// E1 gets despawned on server -> we shouldn't send an extra despawn message to C1
    #[test]
    fn test_redundant_despawn() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::DEBUG)
        //     .init();
        let mut stepper = BevyStepper::default();

        let client = PeerId::Netcode(TEST_CLIENT_ID);
        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                relevance_mode: NetworkRelevanceMode::InterestManagement,
                ..Default::default()
            })
            .id();
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RelevanceManager>()
            .gain_relevance(client, server_entity);

        stepper.frame_step();
        stepper.frame_step();

        // check that entity is replicated, since it's relevant
        let client_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("server entity was not replicated to client");

        // lose relevance, check that entity is despawned
        stepper
            .server_app
            .world_mut()
            .resource_mut::<RelevanceManager>()
            .lose_relevance(client, server_entity);
        stepper.frame_step();
        stepper.frame_step();
        assert!(stepper
            .client_app
            .world()
            .get_entity(client_entity)
            .is_err());

        // despawn entity on the server
        // we shouldn't send an extra Despawn message to the client
        // We check this by making sure that the next_action_message on the receiver channel is 2
        // (because we received one spawn and one despawn)
        stepper.server_app.world_mut().despawn(server_entity);
        stepper.frame_step();
        stepper.frame_step();
        let channel = stepper
            .client_app
            .world()
            .resource::<ClientConnectionManager>()
            .replication_receiver
            .group_channels
            .get(&ReplicationGroupId(server_entity.to_bits()))
            .unwrap();
        assert_eq!(channel.actions_pending_recv_message_id.0, 2);
    }
}
