/*! Main network visibility module, where you can immediately update the network visibility of an entity for a given client

# Network Visibility

The **network visibility** is used to determine which entities are replicated to a client. The server will only replicate the entities that are relevant to a client. If the client stops being
relevant, the server will despawn that entity for that client. This lets you save bandwidth by only sending the necessary data to each client.


Visibility is cached, so after you set an entity as `visible` for a client, it will remain relevant
until you change the setting again.

```rust,no_run
# use bevy_ecs::entity::Entity;
# use bevy_ecs::prelude::World;
# use lightyear_replication::prelude::VisibilityExt;

# let mut client = Entity::from_bits(1);
# let entity = Entity::from_bits(2);
# let mut world = World::new();
world.gain_visibility(entity, client);
world.lose_visibility(entity, client);
```
*/

use bevy_app::prelude::*;
use bevy_derive::Deref;
use bevy_ecs::prelude::*;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::filters_mask::FilterBit;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
#[allow(unused_imports)]
use tracing::{info, trace};

use crate::hierarchy::ReplicateLikeChildren;

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

/// Extension trait for dynamically showing or hiding replicated entities.
///
/// Implemented for both [`World`] (immediate) and [`Commands`] (deferred).
///
/// Visibility changes automatically propagate to descendant entities in the
/// same replication hierarchy (those with [`ReplicateLikeChildren`]).
///
/// # Parameters
///
/// - `entity`: the replicated entity to show or hide.
/// - `sender`: the link entity (connection) for which visibility changes.
///
/// # Example
///
/// ```rust,ignore
/// // Hide an entity from a specific client
/// commands.lose_visibility(server_entity, client_link_entity);
///
/// // Make it visible again
/// commands.gain_visibility(server_entity, client_link_entity);
/// ```
///
/// [`ReplicateLikeChildren`]: crate::hierarchy::ReplicateLikeChildren
pub trait VisibilityExt {
    /// Make `entity` (and its replication-hierarchy descendants) visible to `sender`.
    fn gain_visibility(&mut self, entity: Entity, sender: Entity);

    /// Hide `entity` (and its replication-hierarchy descendants) from `sender`.
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
        if let Some(mut vis) = self.get_mut::<ClientVisibility>(sender) {
            vis.set(entity, bit, true);
        }
        set_replicate_like_children_visibility(self, entity, sender, bit, true);
    }

    fn lose_visibility(&mut self, entity: Entity, sender: Entity) {
        let bit = self.resource::<VisibilityBit>().0;
        if let Some(mut vis) = self.get_mut::<ClientVisibility>(sender) {
            vis.set(entity, bit, false);
        }
        set_replicate_like_children_visibility(self, entity, sender, bit, false);
    }
}

/// Recursively walk [`ReplicateLikeChildren`] and set the visibility bit for each descendant.
fn set_replicate_like_children_visibility(
    world: &mut World,
    entity: Entity,
    sender: Entity,
    bit: FilterBit,
    visible: bool,
) {
    let Some(children) = world.get::<ReplicateLikeChildren>(entity) else {
        return;
    };
    // Copy the entity list to avoid borrowing world while we recurse.
    // ReplicateLikeChildren is typically very small (1-3 entities).
    let child_entities: smallvec::SmallVec<[Entity; 8]> = children.iter().collect();
    for child in child_entities {
        if let Some(mut vis) = world.get_mut::<ClientVisibility>(sender) {
            vis.set(child, bit, visible);
        }
        set_replicate_like_children_visibility(world, child, sender, bit, visible);
    }
}

/// Plugin that handles the visibility system
#[derive(Default)]
pub struct NetworkVisibilityPlugin;

impl Plugin for NetworkVisibilityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VisibilityBit>();
    }
}
