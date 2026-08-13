/*! Main network visibility module, where you can immediately update the network visibility of an entity for a given client

# Network Visibility

The **network visibility** is used to determine which entities are replicated to a client. The
server will only replicate the entities that are relevant to a client. When an entity loses
visibility, it can either be despawned on that client or retained without receiving updates.
This lets you save bandwidth by only sending the necessary data to each client.

Replicon supports three visibility scope lifetimes:

| Scope lifetime | Hidden before first replication | Hidden after replication |
| --- | --- | --- |
| [`ScopeLifetime::WhileVisible`] | Not spawned | Despawned |
| [`ScopeLifetime::AfterFirstVisibility`] | Not spawned | Retained without updates |
| [`ScopeLifetime::AlwaysPresent`] | Spawned, but receives no further updates | Retained without updates |

[`VisibilityExt::lose_visibility`] keeps its common despawning behavior. The two retaining
lifetimes are exposed separately because they differ before the first replication:

- [`VisibilityExt::lose_visibility_retained`] retains an entity only after the client has seen it.
  This is useful for last-known-state views, stable client-side references, or avoiding repeated
  spawn setup when an entity moves in and out of interest.
- [`VisibilityExt::lose_visibility_always_present`] ensures that the entity is spawned even if it
  is already hidden, then pauses further updates. This is useful for identity or hierarchy roots,
  roster entries, objectives, and other placeholders that must exist before their live state is
  relevant. It deliberately reveals the entity's existence and initial state, so it should not be
  used when hidden entities must remain secret.

The retaining policies pause mutations while hidden. The client keeps the last state it received,
and is not automatically notified that the entity became hidden. Applications that need to mark
the state as stale or suspend prediction should send or derive that state separately. Replicon also
does not replay component removals that occur while a retaining scope is hidden, so retained
entities should generally have a stable component layout or an application-level structural resync.

Visibility only controls what happens when an entity becomes hidden. Actually despawning an
authoritative entity still despawns every retained remote copy. To intentionally despawn only on
the sender, remove [`Replicating`](crate::send::Replicating) before despawning the entity.

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
use bevy_ecs::prelude::*;
use bevy_replicon::prelude::ScopeLifetime;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::filters_mask::FilterBit;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
#[allow(unused_imports)]
use tracing::{info, trace};

use crate::hierarchy::ReplicateLikeChildren;
use crate::send::Replicating;

#[doc(hidden)]
#[derive(Resource, Clone, Copy)]
pub struct VisibilityBits {
    while_visible: FilterBit,
    after_first_visibility: FilterBit,
    always_present: FilterBit,
}

impl VisibilityBits {
    fn iter(self) -> impl Iterator<Item = FilterBit> {
        [
            self.while_visible,
            self.after_first_visibility,
            self.always_present,
        ]
        .into_iter()
    }
}

impl FromWorld for VisibilityBits {
    fn from_world(world: &mut World) -> Self {
        let (while_visible, after_first_visibility, always_present) =
            world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
                world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    (
                        filter_registry.register_scope::<Entity>(
                            world,
                            &mut registry,
                            ScopeLifetime::WhileVisible,
                        ),
                        filter_registry.register_scope::<Entity>(
                            world,
                            &mut registry,
                            ScopeLifetime::AfterFirstVisibility,
                        ),
                        filter_registry.register_scope::<Entity>(
                            world,
                            &mut registry,
                            ScopeLifetime::AlwaysPresent,
                        ),
                    )
                })
            });
        Self {
            while_visible,
            after_first_visibility,
            always_present,
        }
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
/// // Pause updates, retaining the entity if this client has already seen it
/// commands.lose_visibility_retained(server_entity, client_link_entity);
///
/// // Ensure the remote entity exists, but pause updates after its initial state
/// commands.lose_visibility_always_present(server_entity, client_link_entity);
///
/// // Make it visible again
/// commands.gain_visibility(server_entity, client_link_entity);
/// ```
///
/// [`ReplicateLikeChildren`]: crate::hierarchy::ReplicateLikeChildren
pub trait VisibilityExt {
    /// Make `entity` (and its replication-hierarchy descendants) visible to `sender`.
    ///
    /// This clears every manual visibility constraint. Other constraints such as replication,
    /// prediction, interpolation, control, rooms, and user-defined visibility filters still apply.
    fn gain_visibility(&mut self, entity: Entity, sender: Entity);

    /// Hide `entity` (and its replication-hierarchy descendants) from `sender`, despawning the
    /// remote entity if it was previously replicated.
    fn lose_visibility(&mut self, entity: Entity, sender: Entity);

    /// Hide `entity` (and its replication-hierarchy descendants) from `sender` while retaining an
    /// existing remote entity without updates.
    ///
    /// If the client has never received the entity, it remains absent until visibility is gained.
    /// Despawning the authoritative entity still despawns its retained remote entity.
    /// Remove [`Replicate`](crate::send::Replicate) first to intentionally suppress that despawn.
    fn lose_visibility_retained(&mut self, entity: Entity, sender: Entity);

    /// Hide `entity` (and its replication-hierarchy descendants) from updates while ensuring that
    /// it is present on `sender`.
    ///
    /// If the client has never received the entity, its initial state is still replicated. Further
    /// updates are paused until visibility is gained. Despawning the authoritative entity still
    /// despawns its retained remote entity. Remove [`Replicate`](crate::send::Replicate) first to
    /// intentionally suppress that despawn.
    fn lose_visibility_always_present(&mut self, entity: Entity, sender: Entity);
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

    fn lose_visibility_retained(&mut self, entity: Entity, sender: Entity) {
        self.queue(move |world: &mut World| {
            world.lose_visibility_retained(entity, sender);
        });
    }

    fn lose_visibility_always_present(&mut self, entity: Entity, sender: Entity) {
        self.queue(move |world: &mut World| {
            world.lose_visibility_always_present(entity, sender);
        });
    }
}

impl VisibilityExt for World {
    fn gain_visibility(&mut self, entity: Entity, sender: Entity) {
        let bits = *self.resource::<VisibilityBits>();
        set_visibility(self, entity, sender, bits, None);
    }

    fn lose_visibility(&mut self, entity: Entity, sender: Entity) {
        let bits = *self.resource::<VisibilityBits>();
        set_visibility(self, entity, sender, bits, Some(bits.while_visible));
    }

    fn lose_visibility_retained(&mut self, entity: Entity, sender: Entity) {
        let bits = *self.resource::<VisibilityBits>();
        set_visibility(
            self,
            entity,
            sender,
            bits,
            Some(bits.after_first_visibility),
        );
    }

    fn lose_visibility_always_present(&mut self, entity: Entity, sender: Entity) {
        let bits = *self.resource::<VisibilityBits>();
        set_visibility(self, entity, sender, bits, Some(bits.always_present));
    }
}

/// Set one manual visibility state and recursively propagate it through [`ReplicateLikeChildren`].
fn set_visibility(
    world: &mut World,
    entity: Entity,
    sender: Entity,
    bits: VisibilityBits,
    hidden_bit: Option<FilterBit>,
) {
    if let Some(mut visibility) = world.get_mut::<ClientVisibility>(sender) {
        // The bits represent alternative states of one logical visibility filter. Clear every
        // previous state before selecting the new one so a shorter lifetime cannot remain active.
        for bit in bits.iter() {
            visibility.set(entity, bit, true);
        }
        if let Some(bit) = hidden_bit {
            visibility.set(entity, bit, false);
        }
    }

    let Some(children) = world.get::<ReplicateLikeChildren>(entity) else {
        return;
    };
    // Copy the entity list to avoid borrowing world while we recurse.
    // ReplicateLikeChildren is typically very small (1-3 entities).
    let child_entities: smallvec::SmallVec<[Entity; 8]> = children.iter().collect();
    for child in child_entities {
        set_visibility(world, child, sender, bits, hidden_bit);
    }
}

/// Makes a real authoritative despawn override Lightyear's retaining visibility scopes.
///
/// Replicon normally suppresses an entity despawn while an `AfterFirstVisibility` or
/// `AlwaysPresent` scope is hidden, because those scopes retain the remote entity when visibility
/// is lost. That is surprising for an actual server-side despawn: visibility loss should retain the
/// remote entity, but ending the authoritative entity's lifecycle should still remove it. Clearing
/// Lightyear's retaining bits here lets Replicon send the authoritative despawn.
///
/// If the server intentionally wants to despawn its local entity without sending a despawn to the
/// client, it should remove [`Replicating`] before despawning, so this observer will not run for
/// that entity.
fn clear_retained_visibility_on_despawn(
    trigger: On<Despawn, Replicating>,
    bits: Res<VisibilityBits>,
    mut senders: Query<&mut ClientVisibility>,
) {
    for mut visibility in &mut senders {
        visibility.set(trigger.entity, bits.after_first_visibility, true);
        visibility.set(trigger.entity, bits.always_present, true);
    }
}

/// Plugin that handles the visibility system
#[derive(Default)]
pub struct NetworkVisibilityPlugin;

impl Plugin for NetworkVisibilityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VisibilityBits>();
        app.add_observer(clear_retained_visibility_on_despawn);
    }
}
