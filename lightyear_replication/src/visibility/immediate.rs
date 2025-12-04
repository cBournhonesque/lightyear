/*! Main network visibility module, where you can immediately update the network visibility of an entity for a given client

# Network Visibility

The **network visibility** is used to determine which entities are replicated to a client. The server will only replicate the entities that are relevant to a client. If the client stops being
relevant, the server will despawn that entity for that client. This lets you save bandwidth by only sending the necessary data to each client.


You can update the [`NetworkVisibility`] component of an entity to control its relevance for specific clients.

The visibility is cached, so after you set an entity as `visible` for a client, it will remain relevant
until you change the setting again.

```rust
# use bevy_ecs::entity::Entity;
# use lightyear_replication::prelude::NetworkVisibility;

# let mut client = Entity::from_bits(1);
let mut visibility = NetworkVisibility::default();
visibility.gain_visibility(client);
visibility.lose_visibility(client);
```
*/

use crate::prelude::ReplicationState;
use crate::send::plugin::ReplicationBufferSystems;
use crate::send::sender::ReplicationSender;
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
#[allow(unused_imports)]
use tracing::info;

/// Event related to [`Entities`](Entity) which are relevant to a client
///
/// The visibility switches to gained/lost/maintained if a visibility function is usedk
#[derive(Debug, PartialEq, Clone, Copy, Default, Reflect)]
pub(crate) enum VisibilityState {
    /// the entity was not replicated to the client, but now is
    Gained,
    /// the entity was replicated to the client, but not anymore
    Lost,
    /// the entity was already replicated to the client, and still is
    Maintained,
    #[default]
    /// the entity is always visible (is not using the visibility system)
    Always,
}

impl VisibilityState {
    /// Returns true if the entity is currently replicated to the client
    pub fn is_visible(&self) -> bool {
        !matches!(self, &VisibilityState::Lost)
    }
}

// TODO: should we store this on the sender entity instead?
//  it would make it faster to 'reset' the visibility every send_interval for each sender
/// Marker component to indicate that interest management is active for this entity.
///
/// We will replicate this entity to the clients specified in the `Replicate` component.
/// On top of that, we will apply interest management logic to determine which peers should receive the entity
///
/// You can use [`gain_visibility`](ReplicationState::gain_visibility) and [`lose_visibility`](NetworkVisibility::lose_visibility)
/// to control the network visibility of entities.
///
/// You can also use [`Room`](super::room::Room)s for a more stateful approach to network visibility
///
/// (the client still needs to be included in the [`Replicate`](crate::prelude::Replicate), the room is simply an additional constraint)
#[derive(Component, Clone, Default, PartialEq, Debug, Reflect)]
#[reflect(Component)]
pub struct NetworkVisibility;

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
    ///
    /// We run this only if the ReplicationSender systems run, otherwise `Gained` would to go `Maintained` and
    /// senders would only see entities as `Maintained`.
    // TODO: this is buggy since the visibility should depend on the sender! Maybe we should store a tick
    //  for when the visibility last changed. Then if the maintained tick is more recent than the previous sender's tick
    //  it means that the entity became visible for the sender, so it should be treated as `Gained`.
    //  Maybe we don't even need `Gained` and `Maintained`. just `Visible(Tick)` and `NotVisible`
    fn update_network_visibility(
        mut query: Query<&mut ReplicationState, With<NetworkVisibility>>,
        manager_query: Query<&ReplicationSender>,
    ) {
        query.iter_mut().for_each(|mut state| {
            state.per_sender_state.retain(|sender_entity, state| {
                if !manager_query
                    .get(*sender_entity)
                    .unwrap()
                    .send_timer
                    .is_finished()
                {
                    return true;
                }
                if state.visibility == VisibilityState::Gained {
                    state.visibility = VisibilityState::Maintained;
                }
                // discard these entities since we already sent a despawn message for it
                if state.visibility == VisibilityState::Lost {
                    return false;
                }
                true
            })
        });
    }
}

#[deprecated(note = "Use VisibilitySystems instead")]
pub type VisibilitySet = VisibilitySystems;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum VisibilitySystems {
    /// Update the [`NetworkVisibility`] components
    UpdateVisibility,
}

impl Plugin for NetworkVisibilityPlugin {
    fn build(&self, app: &mut App) {
        // SYSTEMS
        app.configure_sets(
            PostUpdate,
            VisibilitySystems::UpdateVisibility.in_set(ReplicationBufferSystems::AfterBuffer),
        );
        app.add_systems(
            PostUpdate,
            (Self::update_network_visibility.in_set(VisibilitySystems::UpdateVisibility),),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Broken on main"]
    fn test_network_visibility() {
        let mut app = App::new();
        app.add_plugins(NetworkVisibilityPlugin);
        let entity = app.world_mut().spawn(NetworkVisibility::default()).id();

        let sender = app.world_mut().spawn_empty().id();

        app.world_mut()
            .get_mut::<NetworkVisibility>(entity)
            .unwrap()
            .gain_visibility(sender);
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Gained)
        );

        // after an update: Gained -> Visible
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        // if an entity is already visible, we do not make it Gained
        app.world_mut()
            .get_mut::<NetworkVisibility>(entity)
            .unwrap()
            .gain_visibility(sender);
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Maintained)
        );

        // entity now loses Visibility
        app.world_mut()
            .get_mut::<NetworkVisibility>(entity)
            .unwrap()
            .lose_visibility(sender);
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            Some(&VisibilityState::Lost)
        );

        // after an update: Lost -> Cleared
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            None
        );

        // if we Gain/Lose visibility in the same tick, do nothing
        app.world_mut()
            .get_mut::<NetworkVisibility>(entity)
            .unwrap()
            .gain_visibility(sender);
        app.world_mut()
            .get_mut::<NetworkVisibility>(entity)
            .unwrap()
            .lose_visibility(sender);
        assert_eq!(
            app.world_mut()
                .get_mut::<NetworkVisibility>(entity)
                .unwrap()
                .clients
                .get(&sender),
            None
        );
    }
}
