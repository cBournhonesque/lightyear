//! This module is responsible for making sure that parent-children hierarchies are replicated correctly.
use crate::ReplicationSystems;
use crate::deferred_entity::DeferredEntityCommands;
use crate::prelude::Replicate;
#[cfg(feature = "interpolation")]
use crate::send::InterpolationTarget;
#[cfg(feature = "prediction")]
use crate::send::PredictionTarget;
use crate::visibility::room::{Rooms, RoomsOverridden};
use alloc::vec::Vec;
use bevy_app::prelude::*;
use bevy_ecs::component::Immutable;
use bevy_ecs::entity::{EntityHashMap, MapEntities};
use bevy_ecs::prelude::*;
use bevy_ecs::query::QueryData;
use bevy_ecs::reflect::ReflectMapEntities;
use bevy_ecs::relationship::Relationship;
use bevy_reflect::Reflect;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::postcard_utils;
use bevy_replicon::prelude::{RuleFns, SyncRelatedAppExt};
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, SerializeCtx, WriteCtx};
#[cfg(feature = "client")]
use bevy_replicon::shared::server_entity_map::ServerEntityMap;
use core::fmt::Debug;
use serde::Serialize;
use serde::de::DeserializeOwned;
use smallvec::SmallVec;
use tracing::trace;

#[deprecated(note = "Use RelationshipSystems instead")]
pub type RelationshipSet = RelationshipSystems;
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum RelationshipSystems {
    // PreUpdate
    Receive,
    // PostUpdate
    Send,
}

pub(crate) struct HierarchyPlugin;

/// Client-side placeholder for a replicated [`ChildOf`] whose parent entity
/// has not been mapped yet.
///
/// Replicon's default entity mapping creates a buffered placeholder when a
/// replicated component references an entity that has not appeared in the
/// server-to-client map yet. That is unsafe for relationship components like
/// [`ChildOf`], because inserting the relationship immediately runs Bevy's
/// relationship hooks and leaves Replicon's placeholder buffer alive while the
/// next component in the same entity bundle is decoded.
#[derive(Component)]
pub(crate) struct PendingChildOf {
    server_parent: Entity,
}

impl PendingChildOf {
    fn new(server_parent: Entity) -> Self {
        Self { server_parent }
    }
}

#[derive(QueryData)]
struct PropagationQuery {
    replicate: &'static Replicate,
    #[cfg(feature = "prediction")]
    prediction: Option<&'static PredictionTarget>,
    #[cfg(feature = "interpolation")]
    interpolation: Option<&'static InterpolationTarget>,
}

#[derive(QueryData)]
struct ChildPropagationQuery {
    replicate_like: &'static ReplicateLike,
    replicate: Option<&'static Replicate>,
    cloned: Has<ReplicateCloned>,
    #[cfg(feature = "prediction")]
    prediction: Option<&'static PredictionTarget>,
    #[cfg(feature = "prediction")]
    prediction_cloned: Has<PredictionCloned>,
    #[cfg(feature = "interpolation")]
    interpolation: Option<&'static InterpolationTarget>,
    #[cfg(feature = "interpolation")]
    interpolation_cloned: Has<InterpolationCloned>,
}

impl HierarchyPlugin {
    fn propagate_when_replicate_like_added(
        trigger: On<Insert, ReplicateLike>,
        child_query: Query<ChildPropagationQuery>,
        root_query: Query<PropagationQuery>,
        mut commands: Commands,
    ) {
        if let Ok(child) = child_query.get(trigger.entity)
            && let Ok(root_propagation) = root_query.get(child.replicate_like.root)
        {
            // Buffer all aspect writes for this member so they flush as one
            // insertion bundle (plus one removal bundle) instead of one
            // archetype move per aspect.
            let mut deferred = DeferredEntityCommands::default();
            // Fresh members clone the root's config (marked); on link replace
            // (reparent, or overlapping walks in one tick) marked clones
            // refresh from the new root. User-authored configs are unmarked
            // (see below) and never touched, so an explicit override survives
            // reparenting.
            sync_inherited_aspect::<Replicate, ReplicateCloned>(
                trigger.entity,
                child.replicate,
                Some(root_propagation.replicate),
                child.cloned,
                &mut deferred,
            );
            // Rooms are handled in `RoomPlugin`: members inherit the root's
            // rooms unless they carry an explicit override (`RoomsOverridden`).
            #[cfg(feature = "prediction")]
            sync_inherited_aspect::<PredictionTarget, PredictionCloned>(
                trigger.entity,
                child.prediction,
                root_propagation.prediction,
                child.prediction_cloned,
                &mut deferred,
            );
            #[cfg(feature = "interpolation")]
            sync_inherited_aspect::<InterpolationTarget, InterpolationCloned>(
                trigger.entity,
                child.interpolation,
                root_propagation.interpolation,
                child.interpolation_cloned,
                &mut deferred,
            );
            deferred.apply(&mut commands);
        }
    }
}

/// Sync one inheritable aspect from the link root, for link-time joins and
/// reparenting.
///
/// `marked` means "this aspect follows the root" — recorded when the system
/// cloned it and kept even when the root currently has no value, so a later
/// re-add re-inherits. Unmarked values are user-owned and never touched.
/// Absence is not sticky: a member without the aspect re-inherits it on the
/// next sync (link, reparent, or root change).
fn sync_inherited_aspect<T, CloneMarker>(
    member: Entity,
    member_value: Option<&T>,
    root_value: Option<&T>,
    marked: bool,
    deferred: &mut DeferredEntityCommands,
) where
    T: Component + Clone + PartialEq,
    CloneMarker: Component + Default,
{
    match (member_value, root_value) {
        // Fresh member (or transient absence): inherit and record provenance.
        (None, Some(root)) => {
            deferred.insert(member, root.clone());
            deferred.insert(member, CloneMarker::default());
        }
        // Marked clone diverging from the (new) root: refresh. The equality
        // guard avoids pointless churn on re-links.
        (Some(current), Some(root)) if marked && current != root => {
            deferred.insert(member, root.clone());
        }
        // Marked clone whose aspect the (new) root dropped: clear the copy
        // but keep the marker so a later re-add re-inherits.
        (Some(_), None) if marked => {
            deferred.remove::<T>(member);
        }
        _ => {}
    }
}

/// Drops the inheritance marker when a user-authored aspect value supersedes a
/// clone.
///
/// A hierarchy clone always equals the link root's current config, so equality
/// against the root distinguishes it from an explicit override (same truth
/// table as rooms reconciliation, inverted): differing values — or a missing
/// link or root value — unmark, so the config is now owned by the user and
/// survives root changes and reparenting; equal values keep the marker.
///
/// `On<Insert>` fires for fresh inserts and replacements alike, so in-place
/// overwrites are caught as well. System mirror writes always equal the root
/// at convergence, so they keep the marker and stop.
fn unmark_inherited_on_manual_insert<T, CloneMarker>(
    trigger: On<Insert, T>,
    marked: Query<Has<CloneMarker>>,
    values: Query<&T>,
    replicate_like: Query<&ReplicateLike>,
    mut commands: Commands,
) where
    T: Component + PartialEq,
    CloneMarker: Component,
{
    if !marked.get(trigger.entity).is_ok_and(|marked| marked) {
        return;
    }
    let Ok(value) = values.get(trigger.entity) else {
        return;
    };
    let inherited = replicate_like
        .get(trigger.entity)
        .ok()
        .and_then(|like| values.get(like.root).ok())
        .is_some_and(|root_value| root_value == value);
    if !inherited {
        commands.entity(trigger.entity).try_remove::<CloneMarker>();
    }
}

/// Push a root's aspect change to the members of its subtree that follow it,
/// so `Replicate`/`PredictionTarget`/`InterpolationTarget` switches propagate
/// continuously like rooms: marked members always converge, and unmarked
/// members without the aspect (re-)inherit it like fresh joins. Only a
/// present unmarked value — an explicit override — is never touched.
///
/// `On<Insert>` fires for fresh inserts and replacements alike, so in-place
/// config switches are covered. Writes on members (system clones and user
/// overrides alike) only update tracking and never fan out: descendants
/// inherit from the ultimate root, whose own push reaches them directly since
/// every live link is flattened to it.
fn push_inherited_aspect_to_members<T, CloneMarker>(
    trigger: On<Insert, T>,
    children: Query<&ReplicateLikeChildren>,
    values: Query<&T>,
    replicate_like: Query<&ReplicateLike>,
    cloned: Query<Has<CloneMarker>>,
    mut commands: Commands,
) where
    T: Component + Clone + PartialEq,
    CloneMarker: Component + Default,
{
    if replicate_like.get(trigger.entity).is_ok() {
        return;
    }
    let Ok(root_value) = values.get(trigger.entity) else {
        return;
    };
    let Ok(members) = children.get(trigger.entity) else {
        return;
    };
    // Copy the entity list: the queued mirrors re-trigger component observers.
    let members: SmallVec<[Entity; 8]> = members.iter().collect();
    for member in members {
        let marked = cloned.get(member).is_ok_and(|cloned| cloned);
        let stale = match values.get(member) {
            // A present unmarked value is user-owned: never touch.
            Ok(_) if !marked => continue,
            Ok(current) => current != root_value,
            // Absence holds no opinion — not even unmarked absence — so it
            // counts as stale: fresh joins, removed overrides, and aspects
            // the root gained after linking all (re-)inherit here.
            Err(_) => true,
        };
        if !stale {
            continue;
        }
        if marked {
            commands.entity(member).insert(root_value.clone());
        } else {
            // Unmarked absence: inherit like a fresh join and record
            // provenance.
            commands
                .entity(member)
                .insert((root_value.clone(), CloneMarker::default()));
        }
    }
}

/// Clear a removed root aspect from marked members, keeping their markers so a
/// later re-add re-inherits. Member-side removals need no handling: absence is
/// transient and re-syncs on the next push, reparent, or link.
fn clear_inherited_aspect_on_members<T, CloneMarker>(
    trigger: On<Remove, T>,
    children: Query<&ReplicateLikeChildren>,
    replicate_like: Query<&ReplicateLike>,
    cloned: Query<Has<CloneMarker>>,
    mut commands: Commands,
) where
    T: Component,
    CloneMarker: Component,
{
    if replicate_like.get(trigger.entity).is_ok() {
        return;
    }
    let Ok(members) = children.get(trigger.entity) else {
        return;
    };
    let members: SmallVec<[Entity; 8]> = members.iter().collect();
    for member in members {
        if cloned.get(member).is_ok_and(|cloned| cloned) {
            commands.entity(member).try_remove::<T>();
        }
    }
}

impl Plugin for HierarchyPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::propagate_when_replicate_like_added);
        app.add_observer(unmark_inherited_on_manual_insert::<Replicate, ReplicateCloned>);
        app.add_observer(push_inherited_aspect_to_members::<Replicate, ReplicateCloned>);
        #[cfg(feature = "prediction")]
        {
            app.add_observer(
                unmark_inherited_on_manual_insert::<PredictionTarget, PredictionCloned>,
            );
            app.add_observer(
                push_inherited_aspect_to_members::<PredictionTarget, PredictionCloned>,
            );
            app.add_observer(
                clear_inherited_aspect_on_members::<PredictionTarget, PredictionCloned>,
            );
        }
        #[cfg(feature = "interpolation")]
        {
            app.add_observer(
                unmark_inherited_on_manual_insert::<InterpolationTarget, InterpolationCloned>,
            );
            app.add_observer(
                push_inherited_aspect_to_members::<InterpolationTarget, InterpolationCloned>,
            );
            app.add_observer(
                clear_inherited_aspect_on_members::<InterpolationTarget, InterpolationCloned>,
            );
        }
    }
}

/// Serializes the server parent entity targeted by [`ChildOf`].
///
/// Lightyear registers a custom rule for [`ChildOf`] so the receive path can
/// inspect the raw server entity before mapping it. If the parent is not mapped
/// yet, the receiver defers inserting the relationship instead of letting
/// Replicon create a placeholder entity inside the relationship component.
pub(crate) fn serialize_child_of(
    _ctx: &mut SerializeCtx,
    child_of: &ChildOf,
    message: &mut Vec<u8>,
) -> bevy_ecs::error::Result<()> {
    postcard_utils::entity_to_extend_mut(&child_of.parent(), message)?;
    Ok(())
}

/// Deserializes the raw server parent entity for stale-message consumption.
///
/// The active receive path uses [`write_child_of`] so it can defer insertion
/// until the parent is mapped.
pub(crate) fn deserialize_child_of(
    _ctx: &mut WriteCtx,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<ChildOf> {
    let server_parent = postcard_utils::entity_from_buf(message)?;
    Ok(ChildOf(server_parent))
}

/// Receives [`ChildOf`] without using Replicon's placeholder entity mapper.
///
/// If the parent has already been mapped, this inserts the real Bevy hierarchy
/// relationship. If not, it stores [`PendingChildOf`] and waits for
/// [`resolve_pending_child_of`] to attach the relationship once the parent
/// mapping is available.
pub(crate) fn write_child_of(
    ctx: &mut WriteCtx,
    _rule_fns: &RuleFns<ChildOf>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<()> {
    let server_parent = postcard_utils::entity_from_buf(message)?;
    if let Some(&client_parent) = ctx.entity_map.to_client().get(&server_parent) {
        entity.insert(ChildOf(client_parent));
        entity.remove::<PendingChildOf>();
    } else {
        entity.insert(PendingChildOf::new(server_parent));
        entity.remove::<ChildOf>();
    }
    Ok(())
}

pub(crate) fn remove_child_of(_ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
    entity.remove::<ChildOf>();
    entity.remove::<PendingChildOf>();
}

/// Attach delayed hierarchy relationships once Replicon has mapped the parent.
#[cfg(feature = "client")]
pub(crate) fn resolve_pending_child_of(
    entity_map: Option<Res<ServerEntityMap>>,
    pending: Query<(Entity, &PendingChildOf)>,
    mut commands: Commands,
) {
    let Some(entity_map) = entity_map else {
        return;
    };
    for (entity, pending) in &pending {
        let Some(&client_parent) = entity_map.to_client().get(&pending.server_parent) else {
            continue;
        };
        commands
            .entity(entity)
            .insert(ChildOf(client_parent))
            .remove::<PendingChildOf>();
    }
}

/// When the `DisableReplicateHierarchy` marker component is added to an entity, we will stop replicating their children.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct DisableReplicateHierarchy;

/// Marker component that indicates that this entity should be replicated similarly to the entity
/// contained in the component.
///
/// This will be inserted automatically on all children of an entity that has `Replicate`,
/// unless the child has a [`DisableReplicateHierarchy`] component.
#[derive(Component, Clone, MapEntities, Copy, Reflect, PartialEq, Debug)]
#[relationship(relationship_target=ReplicateLikeChildren)]
#[reflect(Component, MapEntities, PartialEq, Debug)]
pub struct ReplicateLike {
    #[entities]
    pub root: Entity,
}

/// Relationship target component associated with [`ReplicateLike`]
#[derive(Component, Debug, Reflect)]
#[relationship_target(relationship=ReplicateLike, linked_spawn)]
#[reflect(Component)]
pub struct ReplicateLikeChildren(Vec<Entity>);

/// Marker for a `Replicate` that was cloned from a replication root (rather
/// than authored by the user).
///
/// Fully automatic and crate-private: [`HierarchyPlugin`] inserts it alongside
/// every clone, [`push_inherited_aspect_to_members`] refreshes marked clones
/// on root changes, the link observer refreshes them on replacement, while
/// [`unmark_inherited_on_manual_insert`] removes it as soon as a
/// user-authored value differs from the root's. This provenance is what lets
/// root changes and link replacement refresh inherited configs without ever
/// clobbering an explicit override — including when overlapping hierarchy
/// walks write the same entity in one tick, where the last-applied link
/// always agrees with the last-applied clone.
///
/// Caveat, shared with rooms: explicitly writing a value equal to the root's
/// current config is indistinguishable from inheriting.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ReplicateCloned;

/// Marker for a `PredictionTarget` that follows the replication root (rather
/// than authored by the user).
///
/// Fully automatic and crate-private, mirroring [`ReplicateCloned`]: marked
/// means "this aspect follows the root", including when the root currently
/// has no value (the copy is cleared but the marker is kept, so a later
/// re-add re-inherits). [`HierarchyPlugin`] inserts it alongside every clone,
/// [`push_inherited_aspect_to_members`] refreshes marked copies on root
/// changes, and [`unmark_inherited_on_manual_insert`] removes it as soon as a
/// user-authored value differs from the root's.
#[cfg(feature = "prediction")]
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PredictionCloned;

/// Marker for an `InterpolationTarget` that follows the replication root
/// (rather than authored by the user).
///
/// Fully automatic and crate-private, mirroring [`ReplicateCloned`]; same
/// lifecycle as the `PredictionTarget` counterpart.
#[cfg(feature = "interpolation")]
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct InterpolationCloned;

/// Plugin that helps lightyear propagate replication components through a relationship.
///
/// The main idea is this:
/// - when `Replicate` is added, we will add a `ReplicateLike` component to all children
///   - we skip any child that have `DisableReplicateHierarchy` and its descendants.
///     Every other child is linked, including children with their own `Replicate`:
///     their own components override the root's config while rooms and
///     visibility are still inherited. For fully independent replication, add
///     [`RoomsOverridden`](crate::visibility::room::RoomsOverridden) and
///     [`VisibilityOverridden`](crate::visibility::immediate::VisibilityOverridden)
///     alongside the child's `Replicate`.
/// - in the replication send system, every entity's own replication components
///   determine how we do the sync: members carry a clone of the root's config
///   (see below), so no root lookup happens at send time.
///   Any replication component (`ComponentReplicationOverrides`, etc.) can be added on the child entity to override the
///   behaviour only for that child
/// - `Replicate`, `PredictionTarget`, and `InterpolationTarget` are cloned to
///   members at link time and re-pushed on every root change, like rooms:
///   marked clones (`ReplicateCloned` and its target counterparts) always
///   converge to the root's current config, while user-authored values are
///   unmarked on write and never touched afterwards. Absence is transient: a
///   member without the aspect re-inherits it on the next sync.
/// - visibility and rooms are inherited through the hierarchy instead of read
///   from the root at send time: members mirror the root's [`Rooms`]
///   and manual visibility is pushed to them. A member with explicitly set rooms
///   or visibility keeps it (recorded with [`RoomsOverridden`]
///   or [`VisibilityOverridden`](crate::visibility::immediate::VisibilityOverridden));
///   remove the marker to re-inherit. Explicitly setting the root's current
///   value is equivalent to inheriting
pub struct HierarchySendPlugin<R: Relationship> {
    marker: core::marker::PhantomData<R>,
}

impl<R: Relationship> Default for HierarchySendPlugin<R> {
    fn default() -> Self {
        Self {
            marker: core::marker::PhantomData,
        }
    }
}

impl<
    R: Relationship
        + Component<Mutability = Immutable>
        + PartialEq
        + Clone
        + Serialize
        + DeserializeOwned,
> Plugin for HierarchySendPlugin<R>
{
    fn build(&self, app: &mut App) {
        // Note: app.replicate::<R>() is called in SharedComponentRegistrationPlugin
        // so that FnsIds match between client and server.
        app.sync_related_entities::<R>();

        // propagate ReplicateLike
        app.add_observer(Self::propagate_replicate_like_replication_marker_removed);
        app.add_systems(
            PostUpdate,
            Self::propagate_through_hierarchy.before(ReplicationSystems::Send),
        );
    }
}

impl<R: Relationship> HierarchySendPlugin<R> {
    /// Propagate certain replication components through the hierarchy.
    /// - If new children are added, `Replicate` is added, we recursively
    ///   go through the descendants and add `ReplicateLike`, `ChildOfSync`, ... if the child does not have
    ///   `DisableReplicateHierarchy`. Children with their own `Replicate` are
    ///   linked too: it acts as an override of the root's config.
    /// - We run this as a system and not an observer because observers cannot handle Children updates very well
    ///   (if we trigger on ChildOf being added, there is no flush between the ChildOf Add hook and the observer
    ///   so the `&Children` query won't be updated (or the component will not exist on the parent yet)
    fn propagate_through_hierarchy(
        mut commands: Commands,
        root_query: Query<
            (Entity, Option<&ReplicateLike>),
            (
                Or<(With<Replicate>, With<ReplicateLike>)>,
                Without<DisableReplicateHierarchy>,
                With<R::RelationshipTarget>,
                Or<(Changed<R::RelationshipTarget>, Added<Replicate>)>,
            ),
        >,
        children_query: Query<&R::RelationshipTarget>,
        // Only children with `DisableReplicateHierarchy` are excluded: every
        // other child gets a `ReplicateLike` link, including children with
        // their own `Replicate` (which then acts as an override of the root's
        // replication config while rooms/visibility are still inherited).
        // Excluding `Replicate` children would freeze their link on reparent
        // and prune their not-yet-attached descendants from the traversal.
        child_state: Query<
            (Option<&ReplicateLike>, Has<RoomsOverridden>),
            Without<DisableReplicateHierarchy>,
        >,
        rooms: Query<&Rooms>,
    ) {
        // Links queued this run. Inserts go through commands, so queries keep
        // showing pre-flush state for the rest of the run: without this map a
        // later walk would resolve through a stale link and queue a
        // conflicting write (last-queued wins arbitrarily). Queued values are
        // always fully resolved roots, so a single lookup suffices.
        let mut pending: EntityHashMap<Entity> = EntityHashMap::default();
        root_query
            .iter()
            .for_each(|(origin, maybe_replicate_like)| {
                let mut root = origin;
                // If we are already ReplicateLike another entity, we use it as root
                if let Some(new_root) = pending
                    .get(&origin)
                    .copied()
                    .or(maybe_replicate_like.map(|like| like.root))
                {
                    root = new_root;
                }

                // we go through all the descendants (instead of just the children) so that the root is added
                // and we don't need to search for the root ancestor in the replication systems
                let mut stack = SmallVec::<[Entity; 8]>::new();
                stack.push(root);
                while let Some(parent) = stack.pop() {
                    for child in children_query.relationship_sources(parent) {
                        let Ok((existing, overridden)) = child_state.get(child) else {
                            continue;
                        };
                        // Effective link: this run's queued write wins over
                        // pre-flush query state (see `pending` above).
                        let current = pending
                            .get(&child)
                            .copied()
                            .or(existing.map(|like| like.root));
                        // Only write `ReplicateLike` when it is missing or points
                        // at a previous root: re-inserting the same value would
                        // pointlessly re-fire every `ReplicateLike` observer.
                        let reparented = current.is_some_and(|r| r != root);
                        if current.is_none() || reparented {
                            // TODO: should we buffer those inside a SmallVec for batch insert?
                            trace!("Adding ReplicateLike to child {child:?} with root {root:?}.");
                            commands.entity(child).insert(ReplicateLike { root });
                            pending.insert(child, root);
                        }
                        if reparented && !overridden {
                            // The child carries a stale mirror of the previous
                            // root's rooms: force-sync to the new root. Room
                            // reconciliation converges the marker from the
                            // queued write. Marked members keep their override.
                            match rooms.get(root).ok().cloned() {
                                Some(root_rooms) => {
                                    commands.entity(child).insert(root_rooms);
                                }
                                None => {
                                    commands.entity(child).try_remove::<Rooms>();
                                }
                            }
                        }
                        stack.push(child);
                    }
                }
            })
    }

    // TODO: but are the children's despawn replicated? or maybe there's no need because the root's despawned
    //  is replicated, and despawns are recursive
    /// If `Replicate` is removed on an entity that has `Children`
    /// then we remove `ReplicateLike(Entity)` on all the descendants.
    ///
    /// Note that this doesn't happen if the `DisableReplicateHierarchy` is present.
    ///
    /// If a child entity already has the `Replicate` component, we ignore it and its descendants.
    pub(crate) fn propagate_replicate_like_replication_marker_removed(
        trigger: On<Remove, Replicate>,
        root_query: Query<
            (),
            (
                With<R::RelationshipTarget>,
                Without<DisableReplicateHierarchy>,
                With<Replicate>,
            ),
        >,
        children_query: Query<&R::RelationshipTarget>,
        // Strip the link from every linked descendant. Members keep their
        // (possibly cloned) `Replicate` and fall back to independent
        // replication with their last inherited rooms/visibility.
        child_filter: Query<(), With<ReplicateLike>>,
        mut commands: Commands,
    ) {
        let root = trigger.entity;
        // if `DisableReplicateHierarchy` is present, return early since we don't need to propagate `ReplicateLike`
        let Ok(()) = root_query.get(root) else { return };
        let children = children_query.get(root).unwrap();
        // we go through all the descendants (instead of just the children) so that the root is added
        // and we don't need to search for the root ancestor in the replication systems
        let mut stack = SmallVec::<[Entity; 8]>::new();
        stack.push(root);
        while let Some(parent) = stack.pop() {
            for child in children_query.relationship_sources(parent) {
                // Descend regardless so no linked descendant is missed.
                stack.push(child);
                if child_filter.get(child).is_ok() {
                    commands.entity(child).try_remove::<ReplicateLike>();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::prelude::Replicate;
    use alloc::vec;
    use bevy_replicon::prelude::{AuthMethod, RepliconSharedPlugin};
    use bevy_replicon::server::ServerPlugin;
    use bevy_state::app::StatesPlugin;

    fn app_with_hierarchy_plugin() -> App {
        let mut app = App::default();
        app.add_plugins(StatesPlugin);
        app.init_resource::<bevy_time::Time>();
        app.add_plugins(RepliconSharedPlugin {
            auth_method: AuthMethod::None,
        });
        app.add_plugins(ServerPlugin::default());
        app.add_plugins(HierarchySendPlugin::<ChildOf>::default());
        app.add_plugins(HierarchyPlugin);
        app
    }

    /// Check that ReplicateLike propagation works correctly when Children gets updated
    /// on an entity that has ReplicationMarker
    #[test]
    fn propagate_replicate_like_to_all_children() {
        let mut app = app_with_hierarchy_plugin();

        let grandparent = app.world_mut().spawn(Replicate::manual(vec![])).id();
        // parent with no ReplicationMarker: ReplicateLike should be propagated
        let grandchild_1 = app.world_mut().spawn_empty().id();
        let child_1 = app.world_mut().spawn_empty().id();
        let parent_1 = app.world_mut().spawn_empty().add_child(child_1).id();

        // parent with ReplicationMarker: it is linked like any other child, and
        // its whole subtree inherits from the ultimate root (flattening)
        let child_2a = app.world_mut().spawn_empty().id();
        let child_2b = app.world_mut().spawn(Replicate::manual(vec![])).id();
        let grandchild_2b = app.world_mut().spawn_empty().id();
        let child_2c = app
            .world_mut()
            .spawn(ReplicateLike { root: grandparent })
            .id();
        let parent_2 = app
            .world_mut()
            .spawn(Replicate::manual(vec![]))
            .add_children(&[child_2a, child_2b, child_2c])
            .id();

        // parent has Replicate::manual(vec![]) and DisableReplicate so ReplicateLike is not propagated
        let child_3a = app.world_mut().spawn_empty().id();
        let child_3b = app
            .world_mut()
            .spawn(ReplicateLike { root: grandparent })
            .id();
        let parent_3 = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), DisableReplicateHierarchy))
            .add_children(&[child_3a, child_3b])
            .id();

        // parent has DisableReplicate so ReplicateLike is not propagated
        let child_4 = app.world_mut().spawn_empty().id();
        let parent_4 = app
            .world_mut()
            .spawn(DisableReplicateHierarchy)
            .add_child(child_4)
            .id();

        // add Children to the entity which already has Replicate::manual(vec![])
        app.world_mut()
            .entity_mut(grandparent)
            .add_children(&[parent_1, parent_2, parent_3, parent_4]);

        // flush commands
        app.update();

        // Add grandchildren which should also get ReplicateLike
        app.world_mut().entity_mut(parent_1).add_child(grandchild_1);
        app.world_mut()
            .entity_mut(child_2b)
            .add_child(grandchild_2b);

        app.update();

        assert_eq!(
            app.world().get::<ReplicateLike>(parent_1).unwrap().root,
            grandparent
        );
        assert_eq!(
            app.world().get::<ReplicateLike>(child_1).unwrap().root,
            grandparent
        );

        // parent_2 is linked despite its own `Replicate`, which it keeps
        // (no provenance marker: the config is user-authored)
        assert_eq!(
            app.world().get::<ReplicateLike>(parent_2).unwrap().root,
            grandparent
        );
        assert!(app.world().get::<Replicate>(parent_2).is_some());
        assert!(app.world().get::<ReplicateCloned>(parent_2).is_none());
        // the whole subtree inherits from the ultimate root (flattening):
        // child_2a clones the grandparent's config (marked as cloned),
        // child_2b keeps its own config as an override
        assert_eq!(
            app.world().get::<ReplicateLike>(child_2a).unwrap().root,
            grandparent
        );
        assert!(app.world().get::<ReplicateCloned>(child_2a).is_some());
        assert_eq!(
            app.world().get::<ReplicateLike>(child_2b).unwrap().root,
            grandparent
        );
        assert!(app.world().get::<Replicate>(child_2b).is_some());
        assert!(app.world().get::<ReplicateCloned>(child_2b).is_none());
        // manually added links are preserved, not overwritten
        assert_eq!(
            app.world().get::<ReplicateLike>(child_2c).unwrap().root,
            grandparent
        );

        assert!(app.world().get::<ReplicateLike>(parent_3).is_none());
        assert!(app.world().get::<ReplicateLike>(child_3a).is_none());
        // the parent had DisableReplicateHierarchy so the existing ReplicateLike is not overwritten
        assert_eq!(
            app.world().get::<ReplicateLike>(child_3b).unwrap().root,
            grandparent
        );

        // DisableReplicateHierarchy means that ReplicateLike is not propagated and is not added
        // on the entity itself either
        assert!(app.world().get::<ReplicateLike>(parent_4).is_none());
        assert!(app.world().get::<ReplicateLike>(child_4).is_none());

        // The grandchild should replicate like its parent -> grandparent
        assert_eq!(
            app.world().get::<ReplicateLike>(grandchild_1).unwrap().root,
            grandparent
        );
        // ...even below a child with its own `Replicate`: overrides are
        // per-entity, inheritance always comes from the ultimate root
        assert_eq!(
            app.world()
                .get::<ReplicateLike>(grandchild_2b)
                .unwrap()
                .root,
            grandparent
        );
    }

    /// Check that ReplicateLike propagation works correctly when ReplicationMarker gets added
    /// on an entity that already has children
    #[test]
    fn propagate_replicate_like_replication_marker_added() {
        let mut app = app_with_hierarchy_plugin();

        let grandparent = app.world_mut().spawn_empty().id();
        // parent with no ReplicationMarker: ReplicateLike should be propagated
        let child_1 = app.world_mut().spawn_empty().id();
        let parent_1 = app.world_mut().spawn_empty().add_child(child_1).id();

        // parent with ReplicationMarker: the root ReplicateLike shouldn't be propagated
        // but the intermediary ReplicateLike should be propagated to child 2a
        let child_2a = app.world_mut().spawn_empty().id();
        let child_2b = app.world_mut().spawn(Replicate::manual(vec![])).id();
        let child_2c = app
            .world_mut()
            .spawn(ReplicateLike { root: grandparent })
            .id();
        let parent_2 = app
            .world_mut()
            .spawn(Replicate::manual(vec![]))
            .add_children(&[child_2a, child_2b, child_2c])
            .id();

        // parent has ReplicationMarker and DisableReplicate so ReplicateLike is not propagated
        let child_3a = app.world_mut().spawn_empty().id();
        let child_3b = app
            .world_mut()
            .spawn(ReplicateLike { root: grandparent })
            .id();
        let parent_3 = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), DisableReplicateHierarchy))
            .add_children(&[child_3a, child_3b])
            .id();

        // parent has DisableReplicate so ReplicateLike is not propagated
        let child_4 = app.world_mut().spawn_empty().id();
        let parent_4 = app
            .world_mut()
            .spawn(DisableReplicateHierarchy)
            .add_child(child_4)
            .id();

        app.world_mut()
            .entity_mut(grandparent)
            .add_children(&[parent_1, parent_2, parent_3, parent_4]);
        // add ReplicationMarker to an entity that already has children
        app.world_mut()
            .entity_mut(grandparent)
            .insert(Replicate::manual(vec![]));

        // flush commands
        app.update();
        assert_eq!(
            app.world().get::<ReplicateLike>(parent_1).unwrap().root,
            grandparent
        );
        assert_eq!(
            app.world().get::<ReplicateLike>(child_1).unwrap().root,
            grandparent
        );

        // parent_2 is linked despite its own `Replicate`, which it keeps
        assert_eq!(
            app.world().get::<ReplicateLike>(parent_2).unwrap().root,
            grandparent
        );
        assert!(app.world().get::<Replicate>(parent_2).is_some());
        assert!(app.world().get::<ReplicateCloned>(parent_2).is_none());
        assert_eq!(
            app.world().get::<ReplicateLike>(child_2a).unwrap().root,
            grandparent
        );
        assert_eq!(
            app.world().get::<ReplicateLike>(child_2b).unwrap().root,
            grandparent
        );
        assert!(app.world().get::<Replicate>(child_2b).is_some());
        assert!(app.world().get::<ReplicateCloned>(child_2b).is_none());
        // manually added links are preserved, not overwritten
        assert_eq!(
            app.world().get::<ReplicateLike>(child_2c).unwrap().root,
            grandparent
        );

        assert!(app.world().get::<ReplicateLike>(parent_3).is_none());
        assert!(app.world().get::<ReplicateLike>(child_3a).is_none());
        // the parent had DisableReplicateHierarchy so the existing ReplicateLike is not overwritten
        assert_eq!(
            app.world().get::<ReplicateLike>(child_3b).unwrap().root,
            grandparent
        );

        // DisableReplicateHierarchy means that ReplicateLike is not propagated and is not added
        // on the entity itself either
        assert!(app.world().get::<ReplicateLike>(parent_4).is_none());
        assert!(app.world().get::<ReplicateLike>(child_4).is_none());
    }

    /// Check that reparenting refreshes an inherited (cloned) `Replicate`
    /// from the new root, while a user-authored `Replicate` is kept as an
    /// override. Clone provenance is tracked with `ReplicateCloned`.
    #[test]
    fn reparent_refreshes_cloned_replicate_but_keeps_override() {
        let mut app = app_with_hierarchy_plugin();

        let root_a = app.world_mut().spawn(Replicate::manual(vec![])).id();
        let root_c = app.world_mut().spawn(Replicate::default()).id();
        let plain = app.world_mut().spawn(ChildOf(root_a)).id();
        // user-authored config (kept as an override through reparenting)
        let over = app
            .world_mut()
            .spawn((ChildOf(root_a), Replicate::manual(vec![])))
            .id();
        for _ in 0..3 {
            app.update();
        }

        // sanity: the plain member cloned root_a's config (marked),
        // the override kept its own (unmarked)
        assert_eq!(
            app.world().get::<Replicate>(plain),
            Some(&Replicate::manual(vec![]))
        );
        assert!(app.world().get::<ReplicateCloned>(plain).is_some());
        assert!(app.world().get::<ReplicateCloned>(over).is_none());

        app.world_mut().entity_mut(plain).insert(ChildOf(root_c));
        app.world_mut().entity_mut(over).insert(ChildOf(root_c));
        for _ in 0..3 {
            app.update();
        }

        // the clone follows the new root; the override travels untouched
        assert_eq!(
            app.world().get::<ReplicateLike>(plain).unwrap().root,
            root_c
        );
        assert_eq!(
            app.world().get::<Replicate>(plain),
            Some(&Replicate::default())
        );
        assert!(app.world().get::<ReplicateCloned>(plain).is_some());
        assert_eq!(app.world().get::<ReplicateLike>(over).unwrap().root, root_c);
        assert_eq!(
            app.world().get::<Replicate>(over),
            Some(&Replicate::manual(vec![]))
        );
        assert!(app.world().get::<ReplicateCloned>(over).is_none());
    }

    /// Lock in the snapshot contract: hierarchy propagation observes
    /// insert/remove, so these components must stay immutable — in-place
    /// mutation would bypass the observers silently.
    #[test]
    fn replication_configs_are_immutable() {
        fn assert_immutable<T: Component<Mutability = Immutable>>() {}
        assert_immutable::<Replicate>();
        #[cfg(feature = "prediction")]
        assert_immutable::<PredictionTarget>();
        #[cfg(feature = "interpolation")]
        assert_immutable::<InterpolationTarget>();
    }

    /// Check that switching the root's `Replicate` in place propagates to
    /// cloned members, while user-authored configs are kept as overrides.
    #[test]
    fn root_replicate_switch_propagates_to_cloned_members() {
        let mut app = app_with_hierarchy_plugin();

        let dummy = app.world_mut().spawn_empty().id();
        let v1 = Replicate::manual(vec![]);
        let v2 = Replicate::default();
        let v3 = Replicate::manual(vec![dummy]);
        let root = app.world_mut().spawn(v1.clone()).id();
        let plain = app.world_mut().spawn(ChildOf(root)).id();
        let over = app.world_mut().spawn((ChildOf(root), v3.clone())).id();
        for _ in 0..3 {
            app.update();
        }

        // sanity: the plain member cloned the root (marked), the override
        // kept its own config (unmarked)
        assert_eq!(app.world().get::<Replicate>(plain), Some(&v1));
        assert!(app.world().get::<ReplicateCloned>(plain).is_some());
        assert_eq!(app.world().get::<Replicate>(over), Some(&v3));
        assert!(app.world().get::<ReplicateCloned>(over).is_none());

        // switch the root config in place (`On<Insert>` fires for replaces too)
        app.world_mut().entity_mut(root).insert(v2.clone());
        app.update();

        // the clone follows the switch; the override is untouched
        assert_eq!(app.world().get::<Replicate>(plain), Some(&v2));
        assert!(app.world().get::<ReplicateCloned>(plain).is_some());
        assert_eq!(app.world().get::<Replicate>(over), Some(&v3));
        assert!(app.world().get::<ReplicateCloned>(over).is_none());
    }

    /// Check that a user overwrite of a clone (in place) unmarks it, so the
    /// next root switch no longer clobbers the explicit override.
    #[test]
    fn manual_overwrite_of_clone_unmarks_and_survives_switch() {
        let mut app = app_with_hierarchy_plugin();

        let dummy = app.world_mut().spawn_empty().id();
        let v1 = Replicate::manual(vec![]);
        let v2 = Replicate::default();
        let v3 = Replicate::manual(vec![dummy]);
        let root = app.world_mut().spawn(v1.clone()).id();
        let plain = app.world_mut().spawn(ChildOf(root)).id();
        for _ in 0..3 {
            app.update();
        }
        assert!(app.world().get::<ReplicateCloned>(plain).is_some());

        // user overwrites the clone in place: the marker must go
        app.world_mut().entity_mut(plain).insert(v3.clone());
        app.update();
        assert_eq!(app.world().get::<Replicate>(plain), Some(&v3));
        assert!(app.world().get::<ReplicateCloned>(plain).is_none());

        // a later root switch leaves the override alone
        app.world_mut().entity_mut(root).insert(v2.clone());
        app.update();
        assert_eq!(app.world().get::<Replicate>(plain), Some(&v3));
        assert!(app.world().get::<ReplicateCloned>(plain).is_none());
    }

    /// Check that a member that loses its cloned `Replicate` re-inherits it
    /// on the next root switch: absence holds no opinion, even unmarked.
    #[test]
    fn removed_clone_reinherits_on_root_switch() {
        let mut app = app_with_hierarchy_plugin();

        let v1 = Replicate::manual(vec![]);
        let v2 = Replicate::default();
        let root = app.world_mut().spawn(v1.clone()).id();
        let plain = app.world_mut().spawn(ChildOf(root)).id();
        for _ in 0..3 {
            app.update();
        }
        assert!(app.world().get::<ReplicateCloned>(plain).is_some());

        app.world_mut().entity_mut(plain).remove::<Replicate>();
        app.update();
        assert!(app.world().get::<Replicate>(plain).is_none());

        app.world_mut().entity_mut(root).insert(v2.clone());
        app.update();
        assert_eq!(app.world().get::<Replicate>(plain), Some(&v2));
        assert!(app.world().get::<ReplicateCloned>(plain).is_some());
    }

    /// Check the full target lifecycle: link-time clone, root switch,
    /// root removal (copy cleared, marker kept), root re-add (re-inherited),
    /// transient member absence, and sticky user overwrite.
    #[cfg(feature = "prediction")]
    #[test]
    fn root_prediction_target_lifecycle_propagates() {
        let mut app = app_with_hierarchy_plugin();

        let dummy = app.world_mut().spawn_empty().id();
        let a = PredictionTarget::default();
        let b = PredictionTarget::manual(vec![]);
        let c = PredictionTarget::manual(vec![dummy]);
        let root = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), a.clone()))
            .id();
        let plain = app.world_mut().spawn(ChildOf(root)).id();
        let over = app.world_mut().spawn((ChildOf(root), c.clone())).id();
        for _ in 0..3 {
            app.update();
        }

        assert_eq!(app.world().get::<PredictionTarget>(plain), Some(&a));
        assert!(app.world().get::<PredictionCloned>(plain).is_some());
        assert_eq!(app.world().get::<PredictionTarget>(over), Some(&c));
        assert!(app.world().get::<PredictionCloned>(over).is_none());

        // root switch: marked follows, override sticks
        app.world_mut().entity_mut(root).insert(b.clone());
        app.update();
        assert_eq!(app.world().get::<PredictionTarget>(plain), Some(&b));
        assert!(app.world().get::<PredictionCloned>(plain).is_some());
        assert_eq!(app.world().get::<PredictionTarget>(over), Some(&c));

        // root removal: marked copy cleared, marker kept; override untouched
        app.world_mut()
            .entity_mut(root)
            .remove::<PredictionTarget>();
        app.update();
        assert!(app.world().get::<PredictionTarget>(plain).is_none());
        assert!(app.world().get::<PredictionCloned>(plain).is_some());
        assert_eq!(app.world().get::<PredictionTarget>(over), Some(&c));

        // root re-add: the kept marker re-inherits
        app.world_mut().entity_mut(root).insert(a.clone());
        app.update();
        assert_eq!(app.world().get::<PredictionTarget>(plain), Some(&a));
        assert_eq!(app.world().get::<PredictionTarget>(over), Some(&c));

        // member absence is transient: the next root switch re-adds
        app.world_mut()
            .entity_mut(plain)
            .remove::<PredictionTarget>();
        app.update();
        assert!(app.world().get::<PredictionTarget>(plain).is_none());
        app.world_mut().entity_mut(root).insert(b.clone());
        app.update();
        assert_eq!(app.world().get::<PredictionTarget>(plain), Some(&b));

        // user overwrite unmarks and survives later switches
        app.world_mut().entity_mut(plain).insert(c.clone());
        app.update();
        assert!(app.world().get::<PredictionCloned>(plain).is_none());
        app.world_mut().entity_mut(root).insert(a.clone());
        app.update();
        assert_eq!(app.world().get::<PredictionTarget>(plain), Some(&c));
        assert!(app.world().get::<PredictionCloned>(plain).is_none());

        // removing the override re-inherits on the next root change too
        // (not just on reparent): unmarked absence holds no opinion
        app.world_mut()
            .entity_mut(plain)
            .remove::<PredictionTarget>();
        app.update();
        assert!(app.world().get::<PredictionTarget>(plain).is_none());
        app.world_mut().entity_mut(root).insert(b.clone());
        app.update();
        assert_eq!(app.world().get::<PredictionTarget>(plain), Some(&b));
        assert!(app.world().get::<PredictionCloned>(plain).is_some());
    }

    /// Check that an aspect the root gains after linking still reaches
    /// existing members: unmarked absence inherits on push.
    #[cfg(feature = "prediction")]
    #[test]
    fn late_added_root_target_reaches_existing_members() {
        let mut app = app_with_hierarchy_plugin();

        let a = PredictionTarget::default();
        let root = app.world_mut().spawn(Replicate::manual(vec![])).id();
        let plain = app.world_mut().spawn(ChildOf(root)).id();
        for _ in 0..3 {
            app.update();
        }
        assert!(app.world().get::<PredictionTarget>(plain).is_none());
        assert!(app.world().get::<PredictionCloned>(plain).is_none());

        app.world_mut().entity_mut(root).insert(a.clone());
        app.update();
        assert_eq!(app.world().get::<PredictionTarget>(plain), Some(&a));
        assert!(app.world().get::<PredictionCloned>(plain).is_some());
    }

    /// Same lifecycle for interpolation targets (second generic instantiation).
    #[cfg(feature = "interpolation")]
    #[test]
    fn root_interpolation_target_lifecycle_propagates() {
        let mut app = app_with_hierarchy_plugin();

        let dummy = app.world_mut().spawn_empty().id();
        let a = InterpolationTarget::default();
        let b = InterpolationTarget::manual(vec![]);
        let c = InterpolationTarget::manual(vec![dummy]);
        let root = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), a.clone()))
            .id();
        let plain = app.world_mut().spawn(ChildOf(root)).id();
        let over = app.world_mut().spawn((ChildOf(root), c.clone())).id();
        for _ in 0..3 {
            app.update();
        }

        assert_eq!(app.world().get::<InterpolationTarget>(plain), Some(&a));
        assert!(app.world().get::<InterpolationCloned>(plain).is_some());
        assert_eq!(app.world().get::<InterpolationTarget>(over), Some(&c));
        assert!(app.world().get::<InterpolationCloned>(over).is_none());

        app.world_mut().entity_mut(root).insert(b.clone());
        app.update();
        assert_eq!(app.world().get::<InterpolationTarget>(plain), Some(&b));
        assert!(app.world().get::<InterpolationCloned>(plain).is_some());
        assert_eq!(app.world().get::<InterpolationTarget>(over), Some(&c));

        app.world_mut()
            .entity_mut(root)
            .remove::<InterpolationTarget>();
        app.update();
        assert!(app.world().get::<InterpolationTarget>(plain).is_none());
        assert!(app.world().get::<InterpolationCloned>(plain).is_some());
        assert_eq!(app.world().get::<InterpolationTarget>(over), Some(&c));

        app.world_mut().entity_mut(root).insert(a.clone());
        app.update();
        assert_eq!(app.world().get::<InterpolationTarget>(plain), Some(&a));
        assert_eq!(app.world().get::<InterpolationTarget>(over), Some(&c));
    }

    /// Check that reparenting refreshes marked targets from the new root —
    /// clearing the copy (but keeping the marker) when the new root has no
    /// target — while user-authored targets travel untouched.
    #[cfg(feature = "prediction")]
    #[test]
    fn reparent_refreshes_marked_prediction_targets() {
        let mut app = app_with_hierarchy_plugin();

        let dummy = app.world_mut().spawn_empty().id();
        let a = PredictionTarget::default();
        let c = PredictionTarget::manual(vec![dummy]);
        let d = PredictionTarget::manual(vec![]);
        let root_a = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), a.clone()))
            .id();
        let root_c = app.world_mut().spawn(Replicate::default()).id();
        let plain = app.world_mut().spawn(ChildOf(root_a)).id();
        let over = app.world_mut().spawn((ChildOf(root_a), c.clone())).id();
        for _ in 0..3 {
            app.update();
        }
        assert!(app.world().get::<PredictionCloned>(plain).is_some());
        assert!(app.world().get::<PredictionCloned>(over).is_none());

        app.world_mut().entity_mut(plain).insert(ChildOf(root_c));
        app.world_mut().entity_mut(over).insert(ChildOf(root_c));
        for _ in 0..3 {
            app.update();
        }

        // the marked copy is cleared (new root has none) but still inherited:
        // the marker is kept
        assert_eq!(
            app.world().get::<ReplicateLike>(plain).unwrap().root,
            root_c
        );
        assert!(app.world().get::<PredictionTarget>(plain).is_none());
        assert!(app.world().get::<PredictionCloned>(plain).is_some());
        // the override travels untouched
        assert_eq!(app.world().get::<PredictionTarget>(over), Some(&c));
        assert!(app.world().get::<PredictionCloned>(over).is_none());

        // ...so a later target on the new root re-inherits the plain member
        app.world_mut().entity_mut(root_c).insert(d.clone());
        app.update();
        assert_eq!(app.world().get::<PredictionTarget>(plain), Some(&d));
        assert_eq!(app.world().get::<PredictionTarget>(over), Some(&c));
    }
}
