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
use bevy_derive::Deref;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::world::DeferredWorld;
use bevy_reflect::Reflect;
#[allow(unused_imports)]
use bevy_replicon::prelude::{
    AppRuleExt, FilterScope, ScopeLifetime, SingleComponent, VisibilityFilter,
};
use bevy_replicon::server::ServerSystems;
use bevy_replicon::server::server_tick::ServerTick;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::filters_mask::FilterBit;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use bevy_time::Time;
use core::ops::Deref;
use lightyear_connection::host::HostClient;
use lightyear_connection::network_target::NetworkTarget;
#[cfg(feature = "server")]
use lightyear_connection::network_topology::NetworkingMetadata;
#[cfg(feature = "server")]
use lightyear_core::id::PeerId;
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

#[derive(Component, Clone, Default, Debug, PartialEq, Reflect)]
#[component(on_insert = ReplicationTarget::<T>::on_insert)]
#[component(on_discard = ReplicationTarget::<T>::on_discard)]
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
    type VisibilityBit: Resource + Deref<Target = FilterBit>;
    type Context: Default + Send;

    fn post_insert(context: &Self::Context, entity_mut: &mut EntityWorldMut);
    fn update_context(context: &mut Self::Context, sender_entity: Entity, host_client: bool);

    fn on_discard(world: DeferredWorld, context: HookContext);
}

/// Marker component that indicates that the entity was replicated
/// from a remote world.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct ReplicatedFrom {
    /// Entity that holds the [`ReplicationReceiver`](crate::receive::ReplicationReceiver) for this entity
    pub receiver: Entity,
}

impl ReplicationTargetT for () {
    type VisibilityBit = ReplicateBit;
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

    fn on_discard(_: DeferredWorld, _: HookContext) {}
}

/// Clear the replication visibility only when `Replicate` is replaced or removed.
///
/// A component hook cannot distinguish those operations from an entity despawn. Marking an entity
/// hidden while it is being despawned makes Replicon suppress the actual despawn message.
fn on_replicate_discard(
    trigger: On<Discard, Replicate>,
    replicate_bit: Res<ReplicateBit>,
    mut senders: Query<&mut ClientVisibility>,
    mut commands: Commands,
) {
    if trigger.trigger().new_archetype.is_none() {
        return;
    }
    let entity = trigger.entity;

    for mut visibility in &mut senders {
        visibility.set(entity, replicate_bit.0, false);
    }

    commands.queue(move |world: &mut World| {
        let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
            return;
        };
        if !entity_mut.contains::<Replicate>() {
            entity_mut.remove::<Replicating>();
        }
    });
}

/// Entity-level visibility for [`Replicate`]
#[doc(hidden)]
#[derive(Resource, Deref)]
pub struct ReplicateBit(FilterBit);

impl FromWorld for ReplicateBit {
    fn from_world(world: &mut World) -> Self {
        let bit = world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
            world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                filter_registry.register_scope::<Entity>(
                    world,
                    &mut registry,
                    ScopeLifetime::WhileVisible,
                )
            })
        });
        Self(bit)
    }
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
    impl ReplicationTargetT for PredictedSend {
        type VisibilityBit = PredictedBit;

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

        fn on_discard(mut world: DeferredWorld, context: HookContext) {
            let visibility_bit = *world.resource::<PredictedBit>().deref();
            if world.get_entity(context.entity).is_err() {
                return;
            }
            let unsafe_world = world.as_unsafe_world_cell();
            let world = unsafe { unsafe_world.world_mut() };
            let mut senders = world.query::<&mut ClientVisibility>();
            for mut visibility in senders.iter_mut(world) {
                visibility.set(context.entity, visibility_bit, false);
            }
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

    /// Component-level visibility for [`PredictedSend`].
    #[doc(hidden)]
    #[derive(Resource, Deref)]
    pub struct PredictedBit(FilterBit);

    impl FromWorld for PredictedBit {
        fn from_world(world: &mut World) -> Self {
            let bit = world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
                world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    filter_registry.register_scope::<SingleComponent<PredictedSend>>(
                        world,
                        &mut registry,
                        ScopeLifetime::WhileVisible,
                    )
                })
            });
            Self(bit)
        }
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
    impl ReplicationTargetT for InterpolatedSend {
        type VisibilityBit = InterpolatedBit;
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

        fn on_discard(mut world: DeferredWorld, context: HookContext) {
            let visibility_bit = *world.resource::<InterpolatedBit>().deref();
            if world.get_entity(context.entity).is_err() {
                return;
            }
            let unsafe_world = world.as_unsafe_world_cell();
            let world = unsafe { unsafe_world.world_mut() };
            let mut senders = world.query::<&mut ClientVisibility>();
            for mut visibility in senders.iter_mut(world) {
                visibility.set(context.entity, visibility_bit, false);
            }
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

    /// Component-level visibility for [`InterpolatedSend`].
    #[doc(hidden)]
    #[derive(Resource, Deref)]
    pub struct InterpolatedBit(FilterBit);

    impl FromWorld for InterpolatedBit {
        fn from_world(world: &mut World) -> Self {
            let bit = world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
                world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    filter_registry.register_scope::<SingleComponent<InterpolatedSend>>(
                        world,
                        &mut registry,
                        ScopeLifetime::WhileVisible,
                    )
                })
            });
            Self(bit)
        }
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
    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        let entity = context.entity;
        let Some(visibility_bit) = world
            .get_resource::<T::VisibilityBit>()
            .map(|bit| *bit.deref())
        else {
            warn!(
                ?entity,
                "Skipping replication target insertion because the visibility resource is missing"
            );
            return;
        };

        let mut post_insert_context = T::Context::default();

        let unsafe_world = world.as_unsafe_world_cell();
        let world = unsafe { unsafe_world.world_mut() };
        let Some(mode) = (unsafe { unsafe_world.world() })
            .get::<Self>(entity)
            .map(|target| target.mode.clone())
        else {
            return;
        };

        match &mode {
            ReplicationMode::SingleSender => {
                let Ok((sender_entity, mut visibility, host_client)) = world
                    .query_filtered::<(Entity, &mut ClientVisibility, Has<HostClient>), Or<(With<ReplicationSender>, With<HostClient>)>>()
                    .single_mut(world)
                else {
                    return;
                };

                T::update_context(&mut post_insert_context, sender_entity, host_client);
                visibility.set(entity, visibility_bit, true);
            }
            #[cfg(feature = "client")]
            ReplicationMode::SingleClient => {
                use bevy_replicon::prelude::ConnectedClient;

                let (sender_entity, host_client) = if let Ok((sender_entity, mut visibility)) =
                    world
                        .query_filtered::<(Entity, &mut ClientVisibility), With<HostClient>>()
                        .single_mut(world)
                {
                    visibility.set(entity, visibility_bit, true);
                    (sender_entity, true)
                } else if let Ok((sender_entity, mut visibility)) = world
                    .query_filtered::<(Entity, &mut ClientVisibility), With<ConnectedClient>>()
                    .single_mut(world)
                {
                    visibility.set(entity, visibility_bit, true);
                    (sender_entity, false)
                } else {
                    return;
                };

                if host_client {
                    let mut endpoints =
                        world.query_filtered::<&mut ClientVisibility, With<ConnectedClient>>();
                    for mut visibility in endpoints.iter_mut(world) {
                        visibility.set(entity, visibility_bit, false);
                    }
                }

                T::update_context(&mut post_insert_context, sender_entity, host_client);
            }
            #[cfg(feature = "server")]
            ReplicationMode::SingleServer(target) => {
                use lightyear_connection::client_of::ClientOf;
                use lightyear_connection::server::Started;
                use lightyear_link::server::Server;
                use tracing::debug;

                let Ok(server) = world
                    .query_filtered::<&Server, With<Started>>()
                    .single(world)
                else {
                    debug!(
                        "Replicated before server actually existed, dont worry this case scenario is handled!"
                    );
                    return;
                };
                let metadata = unsafe { unsafe_world.world() }.resource::<NetworkingMetadata>();
                let all_clients: alloc::vec::Vec<Entity> = server.collection().to_vec();
                trace!(
                    ?entity,
                    ?visibility_bit,
                    num_clients = all_clients.len(),
                    ?target,
                    "SingleServer on_insert: setting visibility"
                );
                for &sender_entity in &all_clients {
                    if let Ok((mut visibility, _)) = world
                        .query_filtered::<(&mut ClientVisibility, Has<HostClient>), (
                            With<ClientOf>,
                            Or<(With<ReplicationSender>, With<HostClient>)>,
                        )>()
                        .get_mut(world, sender_entity)
                    {
                        trace!(?entity, ?sender_entity, "  hiding bit for client");
                        visibility.set(entity, visibility_bit, false);
                    }
                }
                target.apply_targets(
                    all_clients.into_iter(),
                    &metadata.peer_map,
                    &mut |sender_entity: Entity| {
                        let Ok((mut visibility, host_client)) = world
                            .query_filtered::<(&mut ClientVisibility, Has<HostClient>), (
                                With<ClientOf>,
                                Or<(With<ReplicationSender>, With<HostClient>)>,
                            )>()
                            .get_mut(world, sender_entity)
                        else {
                            return;
                        };
                        trace!(?entity, ?sender_entity, "  showing bit for target client");
                        T::update_context(&mut post_insert_context, sender_entity, host_client);
                        visibility.set(entity, visibility_bit, true);
                    },
                );
            }
            ReplicationMode::Sender(sender_entity) => {
                let sender_entity = *sender_entity;
                let Ok((mut visibility, host_client)) = world
                    .query_filtered::<(&mut ClientVisibility, Has<HostClient>), Or<(With<ReplicationSender>, With<HostClient>)>>()
                    .get_mut(world, sender_entity)
                else {
                    return;
                };
                T::update_context(&mut post_insert_context, sender_entity, host_client);
                visibility.set(entity, visibility_bit, true);
            }
            #[cfg(feature = "server")]
            ReplicationMode::Server(_, _) => {
                unimplemented!()
            }
            ReplicationMode::Target(_) => {
                unimplemented!()
            }
            ReplicationMode::Manual(entities) => {
                let all_senders: alloc::vec::Vec<Entity> = world
                    .query_filtered::<Entity, Or<(With<ReplicationSender>, With<HostClient>)>>()
                    .iter(world)
                    .collect();
                for sender_entity in all_senders {
                    if let Ok(mut visibility) = world
                        .query_filtered::<
                            &mut ClientVisibility,
                            Or<(With<ReplicationSender>, With<HostClient>)>,
                        >()
                        .get_mut(world, sender_entity)
                    {
                        visibility.set(entity, visibility_bit, false);
                    }
                }
                for &sender_entity in entities.iter() {
                    let Ok((mut visibility, host_client)) = world
                        .query_filtered::<(&mut ClientVisibility, Has<HostClient>), Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .get_mut(world, sender_entity)
                    else {
                        continue;
                    };
                    T::update_context(&mut post_insert_context, sender_entity, host_client);
                    visibility.set(entity, visibility_bit, true);
                }
            }
        }

        world.commands().queue(move |world: &mut World| {
            let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                return;
            };
            T::post_insert(&post_insert_context, &mut entity_mut);
        });
    }

    fn on_discard(world: DeferredWorld, context: HookContext) {
        T::on_discard(world, context)
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
#[cfg(feature = "server")]
fn emulate_replicate_on_host_client_added(
    trigger: On<Add, HostClient>,
    remote_ids: Query<&lightyear_core::id::RemoteId>,
    replicate_bit: Res<ReplicateBit>,
    mut host_visibilities: Query<
        &mut ClientVisibility,
        Without<bevy_replicon::prelude::ConnectedClient>,
    >,
    #[cfg(feature = "client")] mut remote_visibilities: Query<
        &mut ClientVisibility,
        With<bevy_replicon::prelude::ConnectedClient>,
    >,
    replicates: Query<(Entity, &Replicate, Has<ReplicatedFrom>)>,
    #[cfg(feature = "prediction")] prediction_targets: Query<(
        Entity,
        &PredictionTarget,
        Has<lightyear_core::prediction::Predicted>,
    )>,
    #[cfg(feature = "prediction")] predicted_bit: Res<PredictedBit>,
    #[cfg(feature = "interpolation")] interpolation_targets: Query<(
        Entity,
        &InterpolationTarget,
        Has<lightyear_core::interpolation::Interpolated>,
    )>,
    #[cfg(feature = "interpolation")] interpolated_bit: Res<InterpolatedBit>,
    mut commands: Commands,
) {
    let host_entity = trigger.entity;
    let host_peer_id = remote_ids
        .get(host_entity)
        .ok()
        .map(|remote_id| remote_id.0);
    let mut host_visibility = match host_visibilities.get_mut(host_entity) {
        Ok(visibility) => Some(visibility),
        Err(_) => {
            commands
                .entity(host_entity)
                .insert(ClientVisibility::default());
            None
        }
    };

    for (entity, replicate, has_replicated_from) in &replicates {
        let mut post_insert_context = <() as ReplicationTargetT>::Context::default();
        let targeted = target_includes_host(replicate, host_entity, host_peer_id);

        #[cfg(feature = "client")]
        if matches!(replicate.mode, ReplicationMode::SingleClient) {
            for mut visibility in remote_visibilities.iter_mut() {
                visibility.set(entity, **replicate_bit, false);
            }
        }

        if let Some(host_visibility) = host_visibility.as_deref_mut() {
            host_visibility.set(entity, **replicate_bit, targeted);
        }
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
        if let Some(host_visibility) = host_visibility.as_deref_mut() {
            host_visibility.set(entity, **predicted_bit, targeted);
        }
        if targeted && !predicted {
            commands
                .entity(entity)
                .insert(lightyear_core::prediction::Predicted);
        }
    }

    #[cfg(feature = "interpolation")]
    for (entity, target, interpolated) in &interpolation_targets {
        let targeted = target_includes_host(target, host_entity, host_peer_id);
        if let Some(host_visibility) = host_visibility.as_deref_mut() {
            host_visibility.set(entity, **interpolated_bit, targeted);
        }
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
/// [`ClientVisibility::default`] treats entities and components as visible. Without this backfill,
/// a late-joining client would receive pre-existing entities and prediction/interpolation markers
/// even when their [`NetworkTarget`] excludes that client.
#[cfg(feature = "server")]
pub(crate) fn handle_new_client_visibility(
    trigger: On<Add, ClientVisibility>,
    remote_id_query: Query<&lightyear_core::id::RemoteId>,
    replication_targets: Query<(Entity, &Replicate)>,
    replicate_bit: Res<ReplicateBit>,
    #[cfg(feature = "prediction")] prediction_targets: Query<(Entity, &PredictionTarget)>,
    #[cfg(feature = "prediction")] predicted_bit: Res<PredictedBit>,
    #[cfg(feature = "interpolation")] interpolation_targets: Query<(Entity, &InterpolationTarget)>,
    #[cfg(feature = "interpolation")] interpolated_bit: Res<InterpolatedBit>,
    controlled_entities: Query<(Entity, &crate::control::ControlledBy)>,
    controlled_bit: Res<crate::control::ControlBit>,
    mut visibilities: Query<&mut ClientVisibility>,
) {
    let sender_entity = trigger.entity;
    let Ok(remote_id) = remote_id_query.get(sender_entity) else {
        return;
    };
    let peer_id = remote_id.0;
    trace!(?sender_entity, ?peer_id, "handle_new_client_visibility");

    let Ok(mut visibility) = visibilities.get_mut(sender_entity) else {
        return;
    };

    for (entity, target) in replication_targets.iter() {
        if let ReplicationMode::SingleServer(ref net_target) = target.mode
            && !net_target.targets(&peer_id)
        {
            visibility.set(entity, **replicate_bit, false);
        }
    }

    #[cfg(feature = "prediction")]
    for (entity, target) in prediction_targets.iter() {
        if let ReplicationMode::SingleServer(ref net_target) = target.mode
            && !net_target.targets(&peer_id)
        {
            visibility.set(entity, **predicted_bit, false);
        }
    }

    #[cfg(feature = "interpolation")]
    for (entity, target) in interpolation_targets.iter() {
        if let ReplicationMode::SingleServer(ref net_target) = target.mode
            && !net_target.targets(&peer_id)
        {
            visibility.set(entity, **interpolated_bit, false);
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
        app.add_observer(on_replicate_discard);

        // make sure that any ordering relative to ReplicationSystems is also applied to ServerSystems
        app.configure_sets(
            PostUpdate,
            ServerSystems::Send.in_set(ReplicationSystems::Send),
        );

        app.register_required_components::<Replicate, Replicating>();
        app.init_resource::<ReplicateBit>();
        app.init_resource::<VisibilityBits>();
        #[cfg(feature = "prediction")]
        {
            app.register_required_components::<PredictionTarget, PredictedSend>();
            app.init_resource::<PredictedBit>();
        }
        #[cfg(feature = "interpolation")]
        {
            app.register_required_components::<InterpolationTarget, InterpolatedSend>();
            app.init_resource::<InterpolatedBit>();
        }
    }
}
