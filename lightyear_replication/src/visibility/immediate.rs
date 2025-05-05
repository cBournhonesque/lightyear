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

use crate::prelude::{ReplicateLikeChildren, ReplicationSender};
use crate::send::ReplicationBufferSet;
use bevy::ecs::entity::hash_map::EntityHashMap;
use bevy::ecs::entity::hash_set::EntityHashSet;
use bevy::ecs::entity::EntityIndexSet;
use bevy::platform::collections::hash_map::Entry;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
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

impl VisibilityState {
    /// Returns true if the entity is currently replicated to the client
    pub fn is_visible(&self) -> bool {
        matches!(self, &VisibilityState::Gained | &VisibilityState::Maintained)
    }
}

// TODO: should we store this on the sender entity instead?
//  it would make it faster to 'reset' the visibility every send_interval for each sender
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
    pub(crate) clients: EntityHashMap<VisibilityState>,
}

impl NetworkVisibility {

    pub(crate) fn is_visible(
        &self, sender: Entity,
    ) -> bool {
        self.clients.get(&sender).is_some_and(|v| v.is_visible())
    }

    pub fn gain_visibility(
        &mut self,
        sender: Entity,
    ) {
        self.clients.entry(sender)
            .and_modify(|v| {
                // if the entity was already relevant (Relevance::Maintained), be careful to not set it to
                // Relevance::Gained as it would trigger a duplicate spawn replication action
                if *v != VisibilityState::Maintained {
                    *v = VisibilityState::Gained;
                }
            })
            .or_insert(VisibilityState::Gained);
    }

    pub fn lose_visibility(
        &mut self,
        sender: Entity,
    ) {
        match self.clients.entry(sender) {
            Entry::Occupied(mut e) => {
                if e.get() == &VisibilityState::Gained {
                    // if the entity was already relevant (Relevance::Maintained), be careful to not set it to
                    // Relevance::Gained as it would trigger a duplicate spawn replication action
                    e.remove();
                } else {
                    *(e.get_mut()) = VisibilityState::Lost;
                }
            }
            Entry::Vacant(_) => {}
        }
    }

    /// Update the visibility of the entity after buffer was done
    fn update_visibility(
        &mut self,
        sender: Entity,
    ) {
        match self.clients.entry(sender) {
            Entry::Occupied(mut e) => {
                if *e.get() == VisibilityState::Gained {
                    e.insert(VisibilityState::Maintained);
                }
                if *e.get() == VisibilityState::Lost {
                    e.remove();
                }
            }
            Entry::Vacant(_) => {}
        }
    }
}


/// Plugin that handles the visibility system
#[derive(Default)]
pub struct NetworkVisibilityPlugin;

impl NetworkVisibilityPlugin {

    // TODO: ideally we would run this in the main 'buffer' system (for performance), but bevy currently has limitations where
    //  we cannot get one mutable component from FilteredEntityMut
    //  See: https://discord.com/channels/691052431525675048/1368398098002345984/1368398098002345984
    //
    /// Update the visibility for each replicated entity.
    /// Gained becomes Maintained, Lost becomes cleared.
    fn update_network_visibility(
        mut query: Query<&mut NetworkVisibility>,
        root_query: Query<&ReplicateLikeChildren>,
        mut manager_query: Query<(Entity, &ReplicationSender)>,
    ) {
        manager_query.iter_mut().for_each(|(sender_entity, sender)| {
            if !sender.send_timer.finished() {
                return;
            }
            sender.replicated_entities.iter().for_each(|root_entity| {
                if let Ok(mut vis) = query.get_mut(*root_entity) {
                    vis.update_visibility(sender_entity);
                }
                if let Ok(children) = root_query.get(*root_entity) {
                    for child in children.iter() {
                        if let Ok(mut vis) = query.get_mut(child) {
                            vis.update_visibility(sender_entity);
                        }
                    }
                }
            });

        });
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum VisibilitySet {
    /// Update the [`NetworkVisibility`] components
    UpdateVisibility,
}

impl Plugin for NetworkVisibilityPlugin {
    fn build(&self, app: &mut App) {
        // REFLECT
        app.register_type::<NetworkVisibility>();
        // SYSTEMS
        app.configure_sets(PostUpdate, VisibilitySet::UpdateVisibility.in_set(ReplicationBufferSet::AfterBuffer));
        app.add_systems(
            PostUpdate,
            (
                Self::update_network_visibility.in_set(VisibilitySet::UpdateVisibility),
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
        assert_eq!(app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().clients.get(&sender), Some(&VisibilityState::Gained));

        // after an update: Gained -> Visible
        app.update();
        assert_eq!(app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().clients.get(&sender), Some(&VisibilityState::Maintained));

        // if an entity is already visible, we do not make it Gained
        app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().gain_visibility(sender);
        assert_eq!(app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().clients.get(&sender), Some(&VisibilityState::Maintained));

        // entity now loses Visibility
        app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().lose_visibility(sender);
        assert_eq!(app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().clients.get(&sender), Some(&VisibilityState::Lost));

        // after an update: Lost -> Cleared
        app.update();
        assert_eq!(app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().clients.get(&sender), None);

        // if we Gain/Lose visibility in the same tick, do nothing
        app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().gain_visibility(sender);
        app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().lose_visibility(sender);
        assert_eq!(app.world_mut().get_mut::<NetworkVisibility>(entity).unwrap().clients.get(&sender), None);
    }
}
