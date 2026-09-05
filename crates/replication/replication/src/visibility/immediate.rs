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

use alloc::collections::BTreeMap;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_replicon::prelude::ScopeLifetime;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::filters_mask::FilterBit;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
#[allow(unused_imports)]
use tracing::{info, trace};

use crate::hierarchy::{ReplicateLike, ReplicateLikeChildren};
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
/// same replication hierarchy (those with [`ReplicateLikeChildren`]),
/// including descendants added to the hierarchy afterwards.
///
/// A hierarchy member with explicitly set visibility keeps it: the explicit
/// write is recorded with [`VisibilityOverridden`] and later root changes no
/// longer propagate to it. Remove the marker to re-inherit the root's
/// visibility. Inheritance is per-entity from the ultimate replication root.
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

/// Last manual-visibility write per `(sender, target)`, so hierarchy pulls can
/// replay a root's state.
///
/// Replicon's [`ClientVisibility`] masks are write-only from outside the
/// crate, so pulls and re-inherits replay from this record instead of reading
/// them back. The state space is tiny: every [`VisibilityExt`] call selects
/// one hidden manual bit (`None` = fully visible).
#[derive(Resource, Default, Debug)]
struct ManualVisibilityRecord {
    hidden: BTreeMap<(Entity, Entity), Option<FilterBit>>,
}

/// Marker for hierarchy members whose manual visibility is managed independently.
///
/// Inserted automatically when visibility is explicitly set on an entity with
/// [`ReplicateLike`]. While present, visibility changes on the replication
/// root no longer propagate to this entity. Remove it to re-inherit the
/// root's visibility.
///
/// Like rooms, visibility inheritance is per-entity from the ultimate
/// [`ReplicateLike`] root: marking one member does not affect its unmarked
/// descendants, which keep following the root.
///
/// [`ReplicateLike`]: crate::hierarchy::ReplicateLike
#[derive(Debug, Clone, Copy, PartialEq, Eq, Component)]
pub struct VisibilityOverridden;

impl VisibilityExt for World {
    fn gain_visibility(&mut self, entity: Entity, sender: Entity) {
        let bits = *self.resource::<VisibilityBits>();
        set_visibility(self, entity, sender, bits, None, true);
    }

    fn lose_visibility(&mut self, entity: Entity, sender: Entity) {
        let bits = *self.resource::<VisibilityBits>();
        set_visibility(self, entity, sender, bits, Some(bits.while_visible), true);
    }

    fn lose_visibility_retained(&mut self, entity: Entity, sender: Entity) {
        let bits = *self.resource::<VisibilityBits>();
        set_visibility(
            self,
            entity,
            sender,
            bits,
            Some(bits.after_first_visibility),
            true,
        );
    }

    fn lose_visibility_always_present(&mut self, entity: Entity, sender: Entity) {
        let bits = *self.resource::<VisibilityBits>();
        set_visibility(self, entity, sender, bits, Some(bits.always_present), true);
    }
}

/// Applies one manual-visibility state to a sender's [`ClientVisibility`].
///
/// Only [`VisibilityBits`] are written: every other filter bit (rooms,
/// replication targets, …) is evaluated per-entity by replicon and must not
/// leak across the hierarchy — overridden members especially rely on that.
/// `set` takes `visible`, while the record stores the hidden bit.
fn apply_bits(
    visibility: &mut ClientVisibility,
    entity: Entity,
    bits: VisibilityBits,
    hidden: Option<FilterBit>,
) {
    for bit in bits.iter() {
        visibility.set(entity, bit, true);
    }
    if let Some(bit) = hidden {
        visibility.set(entity, bit, false);
    }
}

/// Set one manual visibility state and recursively propagate it through [`ReplicateLikeChildren`].
///
/// `direct` distinguishes user calls (which opt a hierarchy member out of
/// inheritance via [`VisibilityOverridden`]) from propagated writes (which
/// never mark, and skip marked members).
fn set_visibility(
    world: &mut World,
    entity: Entity,
    sender: Entity,
    bits: VisibilityBits,
    hidden_bit: Option<FilterBit>,
    direct: bool,
) {
    // The bits represent alternative states of one logical visibility filter. Clear every
    // previous state before selecting the new one so a shorter lifetime cannot remain active.
    let applied = if let Some(mut visibility) = world.get_mut::<ClientVisibility>(sender) {
        apply_bits(&mut visibility, entity, bits, hidden_bit);
        true
    } else {
        false
    };
    // Record what was actually set so hierarchy pulls can replay the root's
    // state without reading replicon's write-only masks. Unapplied writes
    // leave no record: the pull default (fully visible) matches main's
    // pre-override behavior.
    if applied && let Some(mut record) = world.get_resource_mut::<ManualVisibilityRecord>() {
        record.hidden.insert((sender, entity), hidden_bit);
    }
    // A direct write on a hierarchy member records an explicit override, so
    // later root changes no longer propagate to it. Direct writes on roots and
    // standalone entities are just state: they propagate normally and, should
    // the entity later join a hierarchy as a plain member, it inherits.
    if direct
        && world.get::<ReplicateLike>(entity).is_some()
        && let Ok(mut entity_mut) = world.get_entity_mut(entity)
    {
        entity_mut.insert(VisibilityOverridden);
    }

    let Some(children) = world.get::<ReplicateLikeChildren>(entity) else {
        return;
    };
    // Copy the entity list to avoid borrowing world while we recurse.
    // ReplicateLikeChildren is typically very small (1-3 entities).
    let child_entities: smallvec::SmallVec<[Entity; 8]> = children.iter().collect();
    for child in child_entities {
        if world.get::<VisibilityOverridden>(child).is_none() {
            set_visibility(world, child, sender, bits, hidden_bit, false);
        }
    }
}

/// Inherit the replication root's manual visibility when an entity joins its
/// hierarchy after visibility was set.
///
/// [`set_visibility`] pushes state to the [`ReplicateLikeChildren`] that exist
/// at call time, but a child spawned (or reparented) later starts fully
/// visible. When [`ReplicateLike`] is inserted we pull the root's live state
/// for every sender and apply it to the joining subtree.
/// (Rooms need no pull here: they are mirrored as components on attach in
/// `RoomPlugin`.)
fn inherit_visibility_on_replicate_like_added(
    trigger: On<Insert, ReplicateLike>,
    replicate_like: Query<&ReplicateLike>,
    children: Query<&ReplicateLikeChildren>,
    overridden: Query<Has<VisibilityOverridden>>,
    mut senders: Query<(Entity, &mut ClientVisibility)>,
    mut record: ResMut<ManualVisibilityRecord>,
    bits: Res<VisibilityBits>,
) {
    let Ok(like) = replicate_like.get(trigger.entity) else {
        return;
    };
    // Collect the joining subtree (typically a single entity), skipping
    // members with an explicit override: inherited state must not clobber
    // theirs, and pulling never marks.
    let mut stack: smallvec::SmallVec<[Entity; 8]> =
        smallvec::SmallVec::from_elem(trigger.entity, 1);
    let mut subtree: smallvec::SmallVec<[Entity; 8]> = smallvec::SmallVec::new();
    while let Some(entity) = stack.pop() {
        if overridden.get(entity).is_ok_and(|overridden| overridden) {
            continue;
        }
        subtree.push(entity);
        if let Ok(grandchildren) = children.get(entity) {
            stack.extend(grandchildren.iter());
        }
    }
    let bits = *bits;
    let root = like.root;
    for (sender, mut visibility) in &mut senders {
        // Replay the root's last manual write (fully visible by default).
        // Member states are recorded too, so a member that later becomes a
        // sub-root replays correctly.
        let hidden = record.hidden.get(&(sender, root)).copied().flatten();
        for entity in &subtree {
            apply_bits(&mut visibility, *entity, bits, hidden);
            record.hidden.insert((sender, *entity), hidden);
        }
    }
}

/// Re-inherit the replication root's manual visibility after
/// [`VisibilityOverridden`] is removed.
fn reinherit_visibility_on_override_removed(
    trigger: On<Remove, VisibilityOverridden>,
    replicate_like: Query<&ReplicateLike>,
    mut senders: Query<(Entity, &mut ClientVisibility)>,
    mut record: ResMut<ManualVisibilityRecord>,
    bits: Res<VisibilityBits>,
) {
    let Ok(like) = replicate_like.get(trigger.entity) else {
        return;
    };
    let bits = *bits;
    let root = like.root;
    let member = trigger.entity;
    // Without an override the member follows the root again. Replaying (rather
    // than just clearing) also drops stale override bits the member may hold
    // for senders the root never mentions.
    for (sender, mut visibility) in &mut senders {
        let hidden = record.hidden.get(&(sender, root)).copied().flatten();
        apply_bits(&mut visibility, member, bits, hidden);
        record.hidden.insert((sender, member), hidden);
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

/// Drop manual-visibility records involving a despawned entity: entity IDs are
/// recycled, so stale `(sender, target)` pairs must not outlive them.
fn cleanup_manual_visibility_on_despawn(
    trigger: On<Despawn>,
    mut record: ResMut<ManualVisibilityRecord>,
) {
    let entity = trigger.entity;
    record
        .hidden
        .retain(|(sender, target), _| *sender != entity && *target != entity);
}

/// Plugin that handles the visibility system
#[derive(Default)]
pub struct NetworkVisibilityPlugin;

impl Plugin for NetworkVisibilityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VisibilityBits>();
        app.init_resource::<ManualVisibilityRecord>();
        app.add_observer(clear_retained_visibility_on_despawn);
        app.add_observer(inherit_visibility_on_replicate_like_added);
        app.add_observer(reinherit_visibility_on_override_removed);
        app.add_observer(cleanup_manual_visibility_on_despawn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_replicon::shared::replication::registry::ReplicationRegistry;

    /// App with visibility inheritance and one fake sender, without networking.
    fn visibility_app() -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<FilterRegistry>();
        app.init_resource::<ReplicationRegistry>();
        app.add_plugins(NetworkVisibilityPlugin);
        let sender = app.world_mut().spawn(ClientVisibility::default()).id();
        (app, sender)
    }

    /// Reads effective visibility from the recorded manual state (the masks
    /// themselves are write-only): hidden iff the manual despawn-on-hide bit
    /// is set. Faithful in tests: nothing else writes these bits here.
    fn hidden(app: &App, sender: Entity, entity: Entity) -> bool {
        let bits = app.world().resource::<VisibilityBits>();
        app.world()
            .resource::<ManualVisibilityRecord>()
            .hidden
            .get(&(sender, entity))
            .copied()
            .flatten()
            == Some(bits.while_visible)
    }

    fn is_overridden(app: &App, entity: Entity) -> bool {
        app.world().get::<VisibilityOverridden>(entity).is_some()
    }

    #[test]
    fn visibility_propagates_to_members() {
        let (mut app, sender) = visibility_app();
        let root = app.world_mut().spawn_empty().id();
        let child = app.world_mut().spawn(ReplicateLike { root }).id();

        app.world_mut().lose_visibility(root, sender);
        assert!(hidden(&app, sender, root));
        assert!(hidden(&app, sender, child));

        app.world_mut().gain_visibility(root, sender);
        assert!(!hidden(&app, sender, root));
        assert!(!hidden(&app, sender, child));
        // Plain propagation never marks.
        assert!(!is_overridden(&app, root));
        assert!(!is_overridden(&app, child));
    }

    #[test]
    fn direct_member_write_overrides_and_sticks() {
        let (mut app, sender) = visibility_app();
        let root = app.world_mut().spawn_empty().id();
        let child = app.world_mut().spawn(ReplicateLike { root }).id();

        // Hide the child directly: later root pushes must skip it.
        app.world_mut().lose_visibility(child, sender);
        assert!(!hidden(&app, sender, root));
        assert!(hidden(&app, sender, child));
        assert!(is_overridden(&app, child));

        app.world_mut().gain_visibility(root, sender);
        assert!(!hidden(&app, sender, root));
        assert!(
            hidden(&app, sender, child),
            "root push should skip the overridden child"
        );

        // Removing the marker re-inherits the root's (visible) state.
        app.world_mut()
            .entity_mut(child)
            .remove::<VisibilityOverridden>();
        assert!(!hidden(&app, sender, child));
        assert!(!is_overridden(&app, child));

        // ...and the child follows the root again afterwards.
        app.world_mut().lose_visibility(root, sender);
        assert!(hidden(&app, sender, child));
    }

    #[test]
    fn overridden_child_replicates_while_root_hidden() {
        let (mut app, sender) = visibility_app();
        let root = app.world_mut().spawn_empty().id();
        let child = app.world_mut().spawn(ReplicateLike { root }).id();

        app.world_mut().lose_visibility(root, sender);
        app.world_mut().gain_visibility(child, sender);

        assert!(hidden(&app, sender, root));
        assert!(!hidden(&app, sender, child));
        assert!(is_overridden(&app, child));
    }

    #[test]
    fn direct_root_write_does_not_mark() {
        let (mut app, sender) = visibility_app();
        let root = app.world_mut().spawn_empty().id();
        let standalone = app.world_mut().spawn_empty().id();

        app.world_mut().lose_visibility(root, sender);
        app.world_mut().lose_visibility(standalone, sender);

        assert!(!is_overridden(&app, root));
        assert!(!is_overridden(&app, standalone));
    }

    #[test]
    fn late_joiners_inherit_unless_marked() {
        let (mut app, sender) = visibility_app();
        let root = app.world_mut().spawn_empty().id();
        app.world_mut().lose_visibility(root, sender);

        let late = app.world_mut().spawn(ReplicateLike { root }).id();
        assert!(hidden(&app, sender, late));
        assert!(!is_overridden(&app, late));

        // A pre-marked joiner opts out of the pull.
        let independent = app
            .world_mut()
            .spawn((ReplicateLike { root }, VisibilityOverridden))
            .id();
        assert!(!hidden(&app, sender, independent));
    }

    #[test]
    fn reparent_adopts_new_root_state() {
        let (mut app, sender) = visibility_app();
        let root_a = app.world_mut().spawn_empty().id();
        let root_b = app.world_mut().spawn_empty().id();
        let child = app.world_mut().spawn(ReplicateLike { root: root_a }).id();

        // Hide under the first root, then move to a pristine root.
        app.world_mut().lose_visibility(root_a, sender);
        assert!(hidden(&app, sender, child));
        app.world_mut()
            .entity_mut(child)
            .insert(ReplicateLike { root: root_b });
        assert!(
            !hidden(&app, sender, child),
            "reparented member should adopt the new root's default-visible state"
        );

        // ...and to a hidden root.
        app.world_mut().lose_visibility(root_b, sender);
        assert!(hidden(&app, sender, child));

        // Marked members keep their state across reparenting.
        app.world_mut().gain_visibility(child, sender);
        app.world_mut()
            .entity_mut(child)
            .insert(ReplicateLike { root: root_a });
        assert!(!hidden(&app, sender, child));
        assert!(is_overridden(&app, child));
    }
}
