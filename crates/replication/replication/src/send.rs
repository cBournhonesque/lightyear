//! Replication send-side: target components, visibility rules, and send scheduling.
//!
//! The main type here is [`Replicate`] (an alias for [`ReplicationTarget<()>`]),
//! which selects the peers that receive an entity. Adding it also inserts the
//! required [`Replicating`] marker. On the server you also typically add
//! [`PredictionTarget`] and [`InterpolationTarget`] to control which clients
//! run prediction or interpolation for that entity.
//!
//! Each link entity (the entity representing a connection to a remote peer)
//! needs a [`ReplicationSender`] component to enable outgoing replication
//! through that link.
use alloc::vec::Vec;
use bevy_app::prelude::*;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::world::DeferredWorld;
use bevy_reflect::Reflect;
#[allow(unused_imports)]
use bevy_replicon::prelude::{
    AppRuleExt, AppVisibilityExt, FilterScope, ScopeLifetime, SingleComponent, VisibilityFilter,
};
use bevy_replicon::server::ServerSystems;
use bevy_replicon::server::server_tick::ServerTick;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use bevy_time::Time;
use lightyear_connection::host::HostClient;
use lightyear_connection::network_target::NetworkTarget;
#[cfg(any(feature = "client", feature = "server"))]
use lightyear_core::id::PeerId;
use lightyear_core::id::RemoteId;
use serde::{Deserialize, Serialize};

#[allow(unused_imports)]
use tracing::{error, trace, warn};

use crate::ReplicationSystems;
use crate::metadata::ReplicationMetadata;
use crate::registry::ComponentRegistry;
use crate::visibility::immediate::VisibilityBits;
#[cfg(feature = "interpolation")]
pub use interpolation::*;
use lightyear_core::prelude::LocalTimeline;
#[cfg(feature = "prediction")]
pub use prediction::*;

/// Controls which peers an entity is replicated to.
///
/// Each [`ReplicationTarget`] stores a `ReplicationMode` that determines
/// the set of link entities (connections) through which the entity will
/// be sent. Most users should use the convenience constructors on
/// [`Replicate`] rather than constructing a mode directly.
#[derive(Clone, Default, Debug, PartialEq, Reflect)]
pub enum ReplicationMode {
    /// Finds the single [`ReplicationSender`] in the world and replicates to it.
    #[default]
    SingleSender,
    #[cfg(feature = "client")]
    /// Replicates to the server (finds the single `Client` entity).
    SingleClient,
    #[cfg(feature = "server")]
    /// Replicates to a subset of clients connected to the single `Server` entity.
    SingleServer(NetworkTarget),
    /// Replicates to one specific link entity.
    Sender(Entity),
    #[cfg(feature = "server")]
    /// Replicates to a subset of clients for a specific server entity.
    Server(Entity, NetworkTarget),
    /// Replicates to all link entities matching the [`NetworkTarget`].
    Target(NetworkTarget),
    /// Replicates to an explicit list of link entities.
    Manual(Vec<Entity>),
}

impl ReplicationMode {
    /// Pure visibility predicate behind the [`VisibilityFilter`] impls on
    /// [`ReplicationTarget`].
    ///
    /// `client` is the link entity being evaluated and `remote` its [`RemoteId`]
    /// (absent if the link has no id yet).
    pub(crate) fn is_visible_for(&self, client: Entity, remote: Option<&RemoteId>) -> bool {
        // Fail closed: links without a `RemoteId` receive nothing. Inserting
        // `RemoteId` re-evaluates (via Replicon's client-insert observer, or
        // via the new-client backfill when `RemoteId` was already present), so
        // the hidden window is transient.
        match self {
            ReplicationMode::SingleSender => remote.is_some(),
            #[cfg(feature = "client")]
            ReplicationMode::SingleClient => {
                remote.is_some_and(|remote| matches!(remote.0, PeerId::Local(_) | PeerId::Server))
            }
            #[cfg(feature = "server")]
            ReplicationMode::SingleServer(target) => {
                remote.is_some_and(|remote| target.targets(&remote.0))
            }
            ReplicationMode::Sender(sender) => remote.is_some_and(|_| client == *sender),
            #[cfg(feature = "server")]
            ReplicationMode::Server(_, _) => {
                unimplemented!()
            }
            ReplicationMode::Target(_) => {
                unimplemented!()
            }
            ReplicationMode::Manual(senders) => remote.is_some_and(|_| senders.contains(&client)),
        }
    }
}

/// Marker component added to a link entity to enable outgoing replication.
///
/// A link entity represents a connection to a remote peer. Adding
/// `ReplicationSender` to it allows the replication systems to send
/// entity data through that connection.
///
/// On the server, this is typically added in the `On<Add, LinkOf>` observer:
///
/// ```rust,ignore
/// fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
///     commands.entity(trigger.entity).insert(ReplicationSender);
/// }
/// ```
#[derive(Component, Default)]
pub struct ReplicationSender;

/// Selects which peers an entity is replicated to.
///
/// Inserting this component also inserts the required [`Replicating`] marker. Removing it changes
/// the entity's visibility and can despawn its remote copies; remove [`Replicating`] instead to
/// pause replication without despawning them.
///
/// The target is evaluated as a Replicon [`VisibilityFilter`](bevy_replicon::prelude::VisibilityFilter):
/// spawning (or replacing) it evaluates visibility against all connected links, and it composes
/// (logical AND) with rooms and manual visibility. The component is immutable, so retarget with
/// `insert`, not mutation.
///
/// # Example
///
/// ```rust
/// # #[cfg(feature = "server")]
/// # {
/// use bevy_app::App;
/// use bevy_ecs::prelude::Entity;
/// use bevy_replicon::prelude::VisibilityFilter;
/// use lightyear_connection::network_target::NetworkTarget;
/// use lightyear_core::id::{PeerId, RemoteId};
/// use lightyear_replication::prelude::*;
/// use lightyear_replication::send::SendPlugin;
///
/// let mut app = App::new();
/// app.add_plugins(SendPlugin);
///
/// let alice = PeerId::Netcode(1);
/// let bob = PeerId::Netcode(2);
///
/// // Spawn an entity replicated to Alice only, and `Replicating` is added
/// // automatically as a required component.
/// let entity = app
///     .world_mut()
///     .spawn(Replicate::to_clients(NetworkTarget::Single(alice)))
///     .id();
/// assert!(app.world().get::<Replicating>(entity).is_some());
///
/// // Replacing the target re-evaluates visibility against all links.
/// app.world_mut()
///     .entity_mut(entity)
///     .insert(Replicate::to_clients(NetworkTarget::All));
///
/// // The same predicate Replicon evaluates per link. Only the link's
/// // `RemoteId` matters, and links without one receive nothing.
/// let link = Entity::PLACEHOLDER;
/// let target = Replicate::to_clients(NetworkTarget::Single(alice));
/// assert!(target.is_visible(link, Some(&RemoteId(alice))));
/// assert!(!target.is_visible(link, Some(&RemoteId(bob))));
/// assert!(!target.is_visible(link, None));
/// # }
/// ```
pub type Replicate = ReplicationTarget<()>;

/// Marker component that enables replication for a sender-side entity.
///
/// [`Replicate`] inserts this component automatically. Remove it to pause all replication messages
/// for the entity, including its despawn, while keeping [`Replicate`]'s target configuration. Insert
/// it again to resume replication.
///
/// Component removals that happen while replication is paused are not replayed automatically when
/// replication resumes.
pub use bevy_replicon::prelude::Replicated as Replicating;

/// Replication target configuration, evaluated as a Replicon [`VisibilityFilter`].
///
/// The component is immutable: replace it (via `insert`) to retarget, which
/// re-evaluates visibility against all links while preserving manual visibility
/// overrides. It composes (logical AND) with rooms and manual visibility.
#[derive(Component, Clone, Default, Debug, PartialEq, Reflect)]
#[component(immutable, on_insert = ReplicationTarget::<T>::on_insert)]
#[component(on_remove = ReplicationTarget::<T>::on_remove)]
pub struct ReplicationTarget<T: ReplicationTargetT> {
    mode: ReplicationMode,
    #[reflect(ignore)]
    marker: core::marker::PhantomData<T>,
}

mod private {
    pub trait Sealed {}
    impl Sealed for () {}
    #[cfg(feature = "prediction")]
    impl Sealed for super::prediction::PredictedSend {}
    #[cfg(feature = "interpolation")]
    impl Sealed for super::interpolation::InterpolatedSend {}
}

#[doc(hidden)]
pub trait ReplicationTargetT: private::Sealed + Send + Sync + 'static {
    type Context: Default + Send;

    fn post_insert(context: &Self::Context, entity_mut: &mut EntityWorldMut);
    fn update_context(context: &mut Self::Context, sender_entity: Entity, host_client: bool);

    fn on_remove(world: DeferredWorld, context: HookContext);
}

/// Marker component that indicates that the entity was replicated
/// from a remote world.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct ReplicatedFrom {
    /// Entity that holds the [`ReplicationReceiver`](crate::receive::ReplicationReceiver) for this entity
    pub receiver: Entity,
}

/// Entity-level replication target: visible (spawned) exactly on the links in the target.
///
/// `RemoteId` is inserted on a link before its `ClientVisibility`, so links that
/// join after the entity was spawned are covered by
/// [`handle_new_client_visibility`] instead of Replicon's new-client backfill.
impl VisibilityFilter for ReplicationTarget<()> {
    type ClientComponent = RemoteId;
    type Scope = Entity;
    const LIFETIME: ScopeLifetime = ScopeLifetime::WhileVisible;

    fn is_visible(&self, client: Entity, remote: Option<&RemoteId>) -> bool {
        self.is_visible_for(client, remote)
    }
}

impl ReplicationTargetT for () {
    // Context = the host-sender entity.
    type Context = Option<Entity>;

    fn post_insert(context: &Self::Context, entity_mut: &mut EntityWorldMut) {
        if let Some(host_sender) = *context {
            entity_mut.insert((ReplicatedFrom {
                receiver: host_sender,
            },));
        }
    }

    fn update_context(context: &mut Self::Context, sender_entity: Entity, host_client: bool) {
        if host_client {
            *context = Some(sender_entity);
        }
    }

    fn on_remove(_: DeferredWorld, _: HookContext) {}
}

/// Clear the replication visibility only when `Replicate` is removed.
///
/// `On<Remove>` does not fire on replace (only `On<Discard>` does), so a
/// retarget runs just the filter's re-evaluation, preserving manual visibility
/// overrides. A despawn (`new_archetype` of `None`) is skipped: marking an
/// entity hidden while it is being despawned makes Replicon suppress the
/// actual despawn message.
///
/// The hide runs deferred (after Replicon's own `on_remove`, which releases the
/// filter bit) so removing `Replicate` deterministically despawns the remote
/// entity instead of leaving it retained.
fn on_replicate_remove(trigger: On<Remove, Replicate>, mut commands: Commands) {
    if trigger.trigger().new_archetype.is_none() {
        return;
    }
    let entity = trigger.entity;

    commands.queue(move |world: &mut World| {
        if let Some(bit) = world.resource::<FilterRegistry>().get_bit::<Replicate>() {
            let mut senders = world.query::<&mut ClientVisibility>();
            for mut visibility in senders.iter_mut(world) {
                visibility.set(entity, bit, false);
            }
        }
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<Replicating>();
        }
    });
}

#[cfg(feature = "prediction")]
mod prediction {
    use super::*;
    use bevy_replicon::bytes::Bytes;
    use bevy_replicon::prelude::RuleFns;
    use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
    use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
    use lightyear_core::prediction::Predicted;

    /// Sender-side marker that materializes as [`Predicted`] on the receiver.
    ///
    /// Keeping this distinct from [`Predicted`] prevents an authoritative
    /// sender from being mistaken for a client-side predicted simulation.
    /// [`PredictionTarget`] adds this component automatically.
    #[derive(
        Component, Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize,
    )]
    pub struct PredictedSend;

    /// Controls which clients run client-side prediction for this entity.
    ///
    /// This is the send-side way to enable prediction for an entity. The
    /// receive-side can also opt an entity into prediction by inserting
    /// `Predicted` directly on the received entity.
    ///
    /// Typically set to the owning client so they get a `Predicted` entity,
    /// while other clients receive an `Interpolated` entity instead.
    ///
    /// ```rust,ignore
    /// commands.spawn((
    ///     Replicate::to_clients(NetworkTarget::All),
    ///     PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
    ///     InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
    /// ));
    /// ```
    pub type PredictionTarget = ReplicationTarget<PredictedSend>;

    /// Component-level replication target: `PredictedSend` is visible exactly on the links in the target.
    impl VisibilityFilter for PredictionTarget {
        type ClientComponent = RemoteId;
        type Scope = SingleComponent<PredictedSend>;
        const LIFETIME: ScopeLifetime = ScopeLifetime::WhileVisible;

        fn is_visible(&self, client: Entity, remote: Option<&RemoteId>) -> bool {
            self.is_visible_for(client, remote)
        }
    }

    impl ReplicationTargetT for PredictedSend {
        // Context = the host-sender entity
        type Context = bool;

        fn post_insert(context: &Self::Context, entity_mut: &mut EntityWorldMut) {
            if *context {
                entity_mut.insert(Predicted);
            }
        }

        fn update_context(context: &mut Self::Context, _sender_entity: Entity, host_client: bool) {
            *context |= host_client;
        }

        fn on_remove(mut world: DeferredWorld, context: HookContext) {
            if world.get_entity(context.entity).is_err() {
                return;
            }
            // Deferred so it runs after Replicon's own `on_remove` releases the
            // filter bit; see `on_replicate_remove`. This hook does not run on
            // replace, so no replace guard is needed.
            let entity = context.entity;
            world.commands().queue(move |world: &mut World| {
                let Some(bit) = world
                    .resource::<FilterRegistry>()
                    .get_bit::<PredictionTarget>()
                else {
                    return;
                };
                let mut senders = world.query::<&mut ClientVisibility>();
                for mut visibility in senders.iter_mut(world) {
                    visibility.set(entity, bit, false);
                }
            });
        }
    }

    pub(crate) fn write_predicted(
        ctx: &mut WriteCtx,
        rule_fns: &RuleFns<PredictedSend>,
        entity: &mut DeferredEntity,
        message: &mut Bytes,
    ) -> bevy_ecs::error::Result<()> {
        let _ = rule_fns.deserialize(ctx, message)?;
        entity.insert(Predicted);
        Ok(())
    }

    pub(crate) fn remove_predicted(_ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
        entity.remove::<Predicted>();
    }
}

#[cfg(feature = "interpolation")]
mod interpolation {
    use super::*;
    use bevy_replicon::bytes::Bytes;
    use bevy_replicon::prelude::RuleFns;
    use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
    use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, WriteCtx};
    use lightyear_core::interpolation::Interpolated;

    /// Sender-side marker that materializes as [`Interpolated`] on the receiver.
    ///
    /// Keeping this distinct from [`Interpolated`] prevents an authoritative
    /// sender from being mistaken for a client-side interpolated presentation.
    /// [`InterpolationTarget`] adds this component automatically.
    #[derive(
        Component, Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize,
    )]
    pub struct InterpolatedSend;

    /// Controls which clients run server-authoritative interpolation for this entity.
    ///
    /// Typically set to all clients *except* the owning client, so remote
    /// players see a smooth interpolated version of the entity.
    ///
    /// See [`PredictionTarget`] for the complementary prediction setting.
    pub type InterpolationTarget = ReplicationTarget<InterpolatedSend>;

    /// Component-level replication target: `InterpolatedSend` is visible exactly on the links in the target.
    impl VisibilityFilter for InterpolationTarget {
        type ClientComponent = RemoteId;
        type Scope = SingleComponent<InterpolatedSend>;
        const LIFETIME: ScopeLifetime = ScopeLifetime::WhileVisible;

        fn is_visible(&self, client: Entity, remote: Option<&RemoteId>) -> bool {
            self.is_visible_for(client, remote)
        }
    }

    impl ReplicationTargetT for InterpolatedSend {
        // Context = the host-sender entity
        type Context = bool;

        fn post_insert(context: &Self::Context, entity_mut: &mut EntityWorldMut) {
            if *context {
                entity_mut.insert(Interpolated);
            }
        }

        fn update_context(context: &mut Self::Context, _sender_entity: Entity, host_client: bool) {
            *context |= host_client;
        }

        fn on_remove(mut world: DeferredWorld, context: HookContext) {
            if world.get_entity(context.entity).is_err() {
                return;
            }
            // Deferred so it runs after Replicon's own `on_remove` releases the
            // filter bit; see `on_replicate_remove`. This hook does not run on
            // replace, so no replace guard is needed.
            let entity = context.entity;
            world.commands().queue(move |world: &mut World| {
                let Some(bit) = world
                    .resource::<FilterRegistry>()
                    .get_bit::<InterpolationTarget>()
                else {
                    return;
                };
                let mut senders = world.query::<&mut ClientVisibility>();
                for mut visibility in senders.iter_mut(world) {
                    visibility.set(entity, bit, false);
                }
            });
        }
    }

    pub(crate) fn write_interpolated(
        ctx: &mut WriteCtx,
        rule_fns: &RuleFns<InterpolatedSend>,
        entity: &mut DeferredEntity,
        message: &mut Bytes,
    ) -> bevy_ecs::error::Result<()> {
        let _ = rule_fns.deserialize(ctx, message)?;
        entity.insert(Interpolated);
        Ok(())
    }

    pub(crate) fn remove_interpolated(_ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
        entity.remove::<Interpolated>();
    }
}

impl<T: ReplicationTargetT> ReplicationTarget<T> {
    pub fn new(mode: ReplicationMode) -> Self {
        Self {
            mode,
            marker: core::marker::PhantomData,
        }
    }

    #[cfg(feature = "client")]
    pub fn to_server() -> Self {
        Self::new(ReplicationMode::SingleClient)
    }

    #[cfg(feature = "server")]
    pub fn to_clients(target: NetworkTarget) -> Self {
        Self::new(ReplicationMode::SingleServer(target))
    }

    // TODO: small vec
    pub fn manual(senders: Vec<Entity>) -> Self {
        Self::new(ReplicationMode::Manual(senders))
    }
    /// Pure visibility predicate behind the [`VisibilityFilter`] impls.
    ///
    /// Also reused by this hook (host-local markers) and
    /// [`handle_new_client_visibility`] (links that already have a `RemoteId`
    /// when their `ClientVisibility` is inserted) so all three agree.
    pub(crate) fn is_visible_for(&self, client: Entity, remote: Option<&RemoteId>) -> bool {
        self.mode.is_visible_for(client, remote)
    }

    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        let entity = context.entity;
        let unsafe_world = world.as_unsafe_world_cell();
        let Some(mode) = (unsafe { unsafe_world.world() })
            .get::<Self>(entity)
            .map(|target| target.mode.clone())
        else {
            return;
        };

        // Network visibility is owned by the `VisibilityFilter` impl: Replicon
        // evaluates `is_visible` when this component is inserted and whenever a
        // link's `RemoteId` changes. This hook only maintains the host-local
        // receiver markers (`ReplicatedFrom`, `Predicted`, `Interpolated`),
        // which a filter cannot insert.
        let mut post_insert_context = T::Context::default();
        let world = unsafe { unsafe_world.world_mut() };
        let mut hosts = world.query_filtered::<(Entity, Option<&RemoteId>), With<HostClient>>();
        for (sender_entity, remote) in hosts.iter(world) {
            if mode.is_visible_for(sender_entity, remote) {
                T::update_context(&mut post_insert_context, sender_entity, true);
            }
        }

        world.commands().queue(move |world: &mut World| {
            let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                return;
            };
            T::post_insert(&post_insert_context, &mut entity_mut);
        });
    }

    fn on_remove(world: DeferredWorld, context: HookContext) {
        T::on_remove(world, context)
    }
}

pub type ReplicationSendSystems = ServerSystems;

/// Replication is triggered in Replicon every time the `ServerTick` is incremented.
///
/// Run this once per app frame after the fixed loop has drained. Replicating
/// from every app frame can produce multiple Replicon checkpoints for the same
/// Lightyear fixed tick, while running once per fixed step can produce multiple
/// sends in a catch-up frame. Replicon's tick is only a replication checkpoint;
/// it is mapped back to Lightyear's fixed tick by `ReplicationCheckpointMap`.
fn update_replication_tick(
    time: Res<Time>,
    timeline: Res<LocalTimeline>,
    mut replication_metadata: ResMut<ReplicationMetadata>,
    mut replication_tick: ResMut<ServerTick>,
    mut last_observed_local_tick: Local<Option<u32>>,
    mut pending_send: Local<bool>,
) {
    let new_tick = timeline.tick();
    let new_tick_raw = new_tick.0;
    let fixed_ran_this_frame = last_observed_local_tick
        .map(|previous_tick| new_tick_raw > previous_tick)
        .unwrap_or(new_tick_raw > 0);
    *last_observed_local_tick = Some(new_tick_raw);

    if replication_metadata
        .timer
        .tick(time.delta())
        .just_finished()
    {
        *pending_send = true;
    }

    if !*pending_send || !fixed_ran_this_frame {
        trace!(
            target: "lightyear_debug::timeline",
            kind = "server_replication_tick_skipped",
            schedule = "RunFixedMainLoop",
            sample_point = "AfterFixedMainLoop",
            local_tick = new_tick_raw,
            server_tick = ?replication_tick.get(),
            pending_send = *pending_send,
            fixed_ran_this_frame,
            "replication server tick not advanced"
        );
        return;
    }

    let previous_server_tick = replication_tick.get();
    replication_tick.increment();
    *pending_send = false;
    trace!(
        target: "lightyear_debug::timeline",
        kind = "server_replication_tick",
        schedule = "RunFixedMainLoop",
        sample_point = "AfterFixedMainLoop",
        local_tick = new_tick_raw,
        server_tick = ?replication_tick.get(),
        previous_server_tick = ?previous_server_tick,
        "replication server tick advanced"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::schedule::common_conditions::resource_changed;
    use bevy_time::Time;
    use core::time::Duration;

    #[derive(Resource, Default)]
    struct SendCount(usize);

    fn count_sends(mut count: ResMut<SendCount>) {
        count.0 += 1;
    }

    fn add_replication_tick_test_systems(app: &mut App) {
        app.add_systems(
            RunFixedMainLoop,
            update_replication_tick.in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
        );
        app.add_systems(
            PostUpdate,
            count_sends.run_if(resource_changed::<ServerTick>),
        );
    }

    fn clear_send_state(app: &mut App) {
        app.world_mut().run_schedule(PostUpdate);
        app.world_mut().resource_mut::<SendCount>().0 = 0;
        app.world_mut().clear_trackers();
    }

    #[test]
    fn unchanged_timeline_does_not_trigger_send() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.insert_resource(LocalTimeline::default());
        app.insert_resource(ReplicationMetadata::default());
        app.init_resource::<ServerTick>();
        app.init_resource::<SendCount>();
        add_replication_tick_test_systems(&mut app);
        clear_send_state(&mut app);

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(1));
        app.world_mut().run_schedule(RunFixedMainLoop);
        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(app.world().resource::<ServerTick>().get(), 0);
        assert_eq!(app.world().resource::<SendCount>().0, 0);
    }

    #[test]
    fn multiple_fixed_ticks_trigger_single_replication_tick_after_fixed_loop() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.insert_resource(LocalTimeline::default());
        app.insert_resource(ReplicationMetadata::default());
        app.init_resource::<ServerTick>();
        app.init_resource::<SendCount>();
        add_replication_tick_test_systems(&mut app);
        clear_send_state(&mut app);

        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(2);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(1));
        app.world_mut().run_schedule(RunFixedMainLoop);
        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(app.world().resource::<ServerTick>().get(), 1);
        assert_eq!(app.world().resource::<SendCount>().0, 1);
    }

    #[test]
    fn elapsed_interval_on_no_fixed_frame_sends_on_next_fixed_frame() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.insert_resource(LocalTimeline::default());
        app.insert_resource(ReplicationMetadata::new(Duration::from_millis(10)));
        app.init_resource::<ServerTick>();
        app.init_resource::<SendCount>();
        add_replication_tick_test_systems(&mut app);
        clear_send_state(&mut app);

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(10));
        app.world_mut().run_schedule(RunFixedMainLoop);
        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(app.world().resource::<ServerTick>().get(), 0);
        assert_eq!(app.world().resource::<SendCount>().0, 0);

        app.world_mut().clear_trackers();
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(1);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(1));
        app.world_mut().run_schedule(RunFixedMainLoop);
        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(app.world().resource::<ServerTick>().get(), 1);
        assert_eq!(app.world().resource::<SendCount>().0, 1);
    }
}

pub struct SendPlugin;

#[cfg(feature = "server")]
fn target_includes_host<T: ReplicationTargetT>(
    target: &ReplicationTarget<T>,
    host_entity: Entity,
    host_peer_id: Option<PeerId>,
) -> bool {
    match &target.mode {
        #[cfg(feature = "client")]
        ReplicationMode::SingleClient => true,
        ReplicationMode::SingleServer(network_target) => {
            host_peer_id.is_some_and(|peer_id| network_target.targets(&peer_id))
        }
        ReplicationMode::Sender(sender_entity) => *sender_entity == host_entity,
        ReplicationMode::Manual(senders) => senders.contains(&host_entity),
        ReplicationMode::SingleSender
        | ReplicationMode::Target(_)
        | ReplicationMode::Server(_, _) => false,
    }
}

/// When a client becomes a host client after replicated entities already exist, backfill the
/// host-local receiver state that target insertion would normally have added at spawn time.
///
/// Network visibility itself is owned by the [`VisibilityFilter`] impls (plus
/// [`handle_new_client_visibility`] for the `ClientVisibility` inserted below);
/// this observer only backfills the local markers.
#[cfg(feature = "server")]
fn emulate_replicate_on_host_client_added(
    trigger: On<Add, HostClient>,
    remote_ids: Query<&RemoteId>,
    mut host_visibilities: Query<
        &mut ClientVisibility,
        Without<bevy_replicon::prelude::ConnectedClient>,
    >,
    replicates: Query<(Entity, &Replicate, Has<ReplicatedFrom>)>,
    #[cfg(feature = "prediction")] prediction_targets: Query<(
        Entity,
        &PredictionTarget,
        Has<lightyear_core::prediction::Predicted>,
    )>,
    #[cfg(feature = "interpolation")] interpolation_targets: Query<(
        Entity,
        &InterpolationTarget,
        Has<lightyear_core::interpolation::Interpolated>,
    )>,
    mut commands: Commands,
) {
    let host_entity = trigger.entity;
    let host_peer_id = remote_ids
        .get(host_entity)
        .ok()
        .map(|remote_id| remote_id.0);
    if host_visibilities.get_mut(host_entity).is_err() {
        commands
            .entity(host_entity)
            .insert(ClientVisibility::default());
    }

    for (entity, replicate, has_replicated_from) in &replicates {
        let mut post_insert_context = <() as ReplicationTargetT>::Context::default();
        let targeted = target_includes_host(replicate, host_entity, host_peer_id);

        if !targeted {
            continue;
        }

        <() as ReplicationTargetT>::update_context(&mut post_insert_context, host_entity, true);

        let mut entity_commands = commands.entity(entity);
        if post_insert_context.is_some() && !has_replicated_from {
            entity_commands.insert(ReplicatedFrom {
                receiver: host_entity,
            });
        }
    }

    #[cfg(feature = "prediction")]
    for (entity, target, predicted) in &prediction_targets {
        let targeted = target_includes_host(target, host_entity, host_peer_id);
        if targeted && !predicted {
            commands
                .entity(entity)
                .insert(lightyear_core::prediction::Predicted);
        }
    }

    #[cfg(feature = "interpolation")]
    for (entity, target, interpolated) in &interpolation_targets {
        let targeted = target_includes_host(target, host_entity, host_peer_id);
        if targeted && !interpolated {
            commands
                .entity(entity)
                .insert(lightyear_core::interpolation::Interpolated);
        }
    }
}

/// When a new client gets `ClientVisibility`, set the correct visibility bits for all existing
/// replication targets.
///
/// [`ClientVisibility::default`] treats entities and components as visible. Replicon's own
/// new-client backfill skips links that already have the filter's client component, and
/// `RemoteId` is always inserted before `ClientVisibility` on lightyear links — so without
/// this backfill a late-joining client would receive pre-existing entities and
/// prediction/interpolation markers even when their [`NetworkTarget`] excludes that client.
#[cfg(feature = "server")]
pub(crate) fn handle_new_client_visibility(
    trigger: On<Add, ClientVisibility>,
    remote_id_query: Query<&RemoteId>,
    registry: Res<FilterRegistry>,
    replication_targets: Query<(Entity, &Replicate)>,
    #[cfg(feature = "prediction")] prediction_targets: Query<(Entity, &PredictionTarget)>,
    #[cfg(feature = "interpolation")] interpolation_targets: Query<(Entity, &InterpolationTarget)>,
    controlled_entities: Query<(Entity, &crate::control::ControlledBy)>,
    controlled_bit: Res<crate::control::ControlBit>,
    mut visibilities: Query<&mut ClientVisibility>,
) {
    let sender_entity = trigger.entity;
    let remote = remote_id_query.get(sender_entity).ok();
    trace!(?sender_entity, ?remote, "handle_new_client_visibility");

    let Ok(mut visibility) = visibilities.get_mut(sender_entity) else {
        return;
    };

    if let Some(bit) = registry.get_bit::<Replicate>() {
        for (entity, target) in replication_targets.iter() {
            visibility.set(entity, bit, target.is_visible_for(sender_entity, remote));
        }
    }

    #[cfg(feature = "prediction")]
    if let Some(bit) = registry.get_bit::<PredictionTarget>() {
        for (entity, target) in prediction_targets.iter() {
            visibility.set(entity, bit, target.is_visible_for(sender_entity, remote));
        }
    }

    #[cfg(feature = "interpolation")]
    if let Some(bit) = registry.get_bit::<InterpolationTarget>() {
        for (entity, target) in interpolation_targets.iter() {
            visibility.set(entity, bit, target.is_visible_for(sender_entity, remote));
        }
    }

    // Hide ControlledSend for entities not owned by this client. Receivers
    // materialize this marker as Controlled.
    for (entity, controlled_by) in controlled_entities.iter() {
        if controlled_by.owner != sender_entity {
            visibility.set(entity, **controlled_bit, false);
        }
    }
}

impl Plugin for SendPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<ComponentRegistry>() {
            app.world_mut().init_resource::<ComponentRegistry>();
        }
        if !app.world().contains_resource::<ReplicationRegistry>() {
            app.world_mut().init_resource::<ReplicationRegistry>();
        }
        if !app.world().contains_resource::<FilterRegistry>() {
            app.world_mut().init_resource::<FilterRegistry>();
        }

        app.add_systems(
            RunFixedMainLoop,
            update_replication_tick
                .in_set(ServerSystems::IncrementTick)
                .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop)
                .run_if(not(lightyear_core::timeline::is_in_rollback)),
        );
        #[cfg(feature = "server")]
        app.add_observer(emulate_replicate_on_host_client_added);
        app.add_observer(on_replicate_remove);

        // make sure that any ordering relative to ReplicationSystems is also applied to ServerSystems
        app.configure_sets(
            PostUpdate,
            ServerSystems::Send.in_set(ReplicationSystems::Send),
        );

        app.register_required_components::<Replicate, Replicating>();
        // Replication targets are `VisibilityFilter`s: Replicon evaluates them on
        // insertion and on link changes, replacing the previous manual bit writes.
        app.add_visibility_filter::<Replicate>();
        app.init_resource::<VisibilityBits>();
        #[cfg(feature = "prediction")]
        {
            app.register_required_components::<PredictionTarget, PredictedSend>();
            app.add_visibility_filter::<PredictionTarget>();
        }
        #[cfg(feature = "interpolation")]
        {
            app.register_required_components::<InterpolationTarget, InterpolatedSend>();
            app.add_visibility_filter::<InterpolationTarget>();
        }
    }
}
