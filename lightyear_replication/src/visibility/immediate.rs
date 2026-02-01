/*! Main network visibility module, where you can immediately update the network visibility of an entity for a given client

# Network Visibility

The **network visibility** is used to determine which entities are replicated to a client. The server will only replicate the entities that are relevant to a client. If the client stops being
relevant, the server will despawn that entity for that client. This lets you save bandwidth by only sending the necessary data to each client.


You can add the [`NetworkVisibility`] component on an entity to indicate that this entity is using the visibility ystem.

The visibility is cached, so after you set an entity as `visible` for a client, it will remain relevant
until you change the setting again.
To control the visibility, you can set it on the [`ReplicationState`] componet.

```rust
# use bevy_ecs::entity::Entity;
# use lightyear_replication::prelude::ReplicationState;

# let mut client = Entity::from_bits(1);
let mut state = ReplicationState::default();
state.gain_visibility(client);
state.lose_visibility(client);
```
*/

use bevy_replicon::server::visibility::filters_mask::FilterBit;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use bevy_app::prelude::*;
use bevy_derive::Deref;
use bevy_ecs::prelude::*;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
#[allow(unused_imports)]
use tracing::{info, trace};



#[doc(hidden)]
#[derive(Resource, Deref)]
pub struct VisibilityBit(FilterBit);

impl FromWorld for VisibilityBit {
    fn from_world(world: &mut World) -> Self {
        let bit = world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
            world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                filter_registry.register_scope::<Entity>(world, &mut registry)
            })
        });
        Self(bit)
    }
}

pub trait VisibilityExt {
    fn gain_visibility(&mut self, entity: Entity, sender: Entity);

    fn lose_visibility(&mut self, entity: Entity, sender: Entity);
}

impl VisibilityExt for Commands<'_, '_> {
    fn gain_visibility(&mut self, entity: Entity, sender: Entity) {
    self.queue(move |world: &mut World| {
            world.gain_visibility(entity, sender);
        });
    }

    fn lose_visibility(&mut self, entity: Entity, sender: Entity) {
        self.queue(move |world: &mut World| {
            world.lose_visibility(entity, sender);
        });
    }
}

impl VisibilityExt for World {
    fn gain_visibility(&mut self, entity: Entity, sender: Entity) {
        let bit = self.resource::<VisibilityBit>().0;
        if let Some(mut client_visibility) = self.get_mut::<ClientVisibility>(sender) {
            client_visibility.set(entity, bit, true);
        }
    }

    fn lose_visibility(&mut self, entity: Entity, sender: Entity) {
        let bit = self.resource::<VisibilityBit>().0;
        if let Some(mut client_visibility) = self.get_mut::<ClientVisibility>(sender) {
            client_visibility.set(entity, bit, false);
        }
    }
}


/// Plugin that handles the visibility system
#[derive(Default)]
pub struct NetworkVisibilityPlugin;

impl Plugin for NetworkVisibilityPlugin {
    fn build(&self, app: &mut App) {
        // SYSTEMS
        app.init_resource::<VisibilityBit>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // #[ignore = "Broken on main"]
    fn test_network_visibility() {
        let mut app = App::new();
        app.add_plugins(NetworkVisibilityPlugin);
        let entity = app.world_mut().spawn(NetworkVisibility::default()).id();

        let sender = app.world_mut().spawn(ReplicationSender::default()).id();

        app.world_mut()
            .get_mut::<ReplicationState>(entity)
            .unwrap()
            .gain_visibility(sender);
        assert_eq!(
            app.world_mut()
                .get_mut::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Gained
        );

        // after an update: Gained -> Visible
        app.update();
        assert_eq!(
            app.world_mut()
                .get_mut::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Visible
        );

        // if an entity is already visible, we do not make it Gained
        app.world_mut()
            .get_mut::<ReplicationState>(entity)
            .unwrap()
            .gain_visibility(sender);
        assert_eq!(
            app.world_mut()
                .get_mut::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Visible
        );

        // entity now loses Visibility
        app.world_mut()
            .get_mut::<ReplicationState>(entity)
            .unwrap()
            .lose_visibility(sender);
        assert_eq!(
            app.world_mut()
                .get_mut::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Lost
        );

        // after an update: Lost -> Cleared
        app.update();
        assert!(
            app.world_mut()
                .get_mut::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .is_none()
        );

        // if we Gain/Lose visibility in the same tick, do nothing
        app.world_mut()
            .get_mut::<ReplicationState>(entity)
            .unwrap()
            .gain_visibility(sender);
        app.world_mut()
            .get_mut::<ReplicationState>(entity)
            .unwrap()
            .lose_visibility(sender);
        assert_eq!(
            app.world_mut()
                .get_mut::<ReplicationState>(entity)
                .unwrap()
                .per_sender_state
                .get(&sender)
                .unwrap()
                .visibility,
            VisibilityState::Default
        );
    }
}
