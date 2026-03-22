//! Handles visibility rules for Replicate, PredictionTarget, and InterpolationTarget components.
use crate::authority::{AuthorityBroker, HasAuthority};
use alloc::vec::Vec;
use bevy_app::prelude::*;
use bevy_derive::Deref;
use bevy_ecs::entity::EntityIndexMap;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::world::DeferredWorld;
use bevy_reflect::Reflect;
#[allow(unused_imports)]
use bevy_replicon::prelude::{
    AppRuleExt, FilterScope, Replicated, SingleComponent, VisibilityFilter,
};
use bevy_replicon::server::ServerSystems;
use bevy_replicon::server::server_tick::ServerTick;
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::filters_mask::FilterBit;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use bevy_replicon::shared::replication::track_mutate_messages::TrackAppExt;
use bevy_time::Time;
use core::ops::Deref;
use lightyear_connection::client::PeerMetadata;
use lightyear_connection::host::HostClient;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::id::PeerId;
use serde::{Deserialize, Serialize};

#[allow(unused_imports)]
use tracing::{error, trace, warn};

use crate::ReplicationSystems;
use crate::metadata::ReplicationMetadata;
use crate::registry::ComponentRegistry;
use crate::visibility::immediate::VisibilityBit;
#[cfg(feature = "interpolation")]
pub use interpolation::*;
use lightyear_core::prelude::LocalTimeline;
#[cfg(feature = "prediction")]
pub use prediction::*;

#[derive(Clone, Default, Debug, PartialEq, Reflect)]
pub enum ReplicationMode {
    /// Will try to find a single ReplicationSender entity in the world
    #[default]
    SingleSender,
    #[cfg(feature = "client")]
    /// Will try to find a single Client entity in the world
    SingleClient,
    #[cfg(feature = "server")]
    /// Will try to find a single Server entity in the world
    SingleServer(NetworkTarget),
    /// Will use this specific entity
    Sender(Entity),
    #[cfg(feature = "server")]
    /// Will use all the clients for that server entity
    Server(Entity, NetworkTarget),
    /// Will assign to various ReplicationSenders to replicate to
    /// all peers in the NetworkTarget
    Target(NetworkTarget),
    Manual(Vec<Entity>),
}

/// Marker component to indicate that this peer should be replicating to its own remote peer
#[derive(Component, Default)]
pub struct ReplicationSender;

/// Insert this component to start replicating your entity.
///
/// Remove it to pause sending replication updates.
/// If you want to despawn an entity without the despawn getting replicated; you need to first remove this component before despawning the entity.
pub type Replicate = ReplicationTarget<()>;

#[derive(Component, Clone, Default, Debug, PartialEq, Reflect)]
#[require(ReplicationState)]
#[component(on_insert = ReplicationTarget::<T>::on_insert)]
#[component(on_replace = ReplicationTarget::<T>::on_replace)]
pub struct ReplicationTarget<T: ReplicationTargetT> {
    mode: ReplicationMode,
    #[reflect(ignore)]
    marker: core::marker::PhantomData<T>,
}

/// Component containins per-[`ReplicationSender`] metadata for the entity.
///
/// This can be used to update the visibility of the entity if [`NetworkVisibility`](crate::visibility::immediate::NetworkVisibility)
/// is present on the entity.
///
/// ```
/// # use bevy_ecs::prelude::*;
/// # use lightyear_replication::prelude::{NetworkVisibility, Replicate, ReplicationState};
/// # let mut world = World::new();
/// # let entity = world.spawn((ReplicationState::default(), NetworkVisibility));
/// # let mut sender = world.spawn_empty();
/// let mut state = world.get_mut::<ReplicationState>(entity).unwrap();
/// // the entity will now be visible (replicated) on that sender
/// state.gain_visibility(sender);
/// // the entity won't be visible for that sender
/// state.lose_visibility(sender);
/// ```
// This is kept separate from the Replicate for situations like:
// - specifying that a sender has no authority over an entity independently even without Replicate being added
#[derive(Component, Default, Debug)]
pub struct ReplicationState {
    /// The list of [`ReplicationSender`] entities that this entity is being replicated on
    pub(crate) per_sender_state: EntityIndexMap<PerSenderReplicationState>,
    // TODO: maybe add ReplicationGroup information here?
}

impl ReplicationState {
    #[cfg(feature = "test_utils")]
    pub fn state(&self) -> &EntityIndexMap<PerSenderReplicationState> {
        &self.per_sender_state
    }

    pub fn has_authority(&self, sender: Entity) -> bool {
        self.per_sender_state
            .get(&sender)
            .is_some_and(|s| s.authority.is_some_and(|a| a))
    }

    pub(crate) fn lose_authority(&mut self, sender: Entity) {
        self.per_sender_state
            .entry(sender)
            .and_modify(|s| s.authority = Some(false))
            .or_insert_with(PerSenderReplicationState::without_authority);
    }

    pub(crate) fn gain_authority(&mut self, sender: Entity) {
        self.per_sender_state
            .entry(sender)
            .and_modify(|s| s.authority = Some(true))
            .or_insert_with(PerSenderReplicationState::with_authority);
    }
}

#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct PerSenderReplicationState {
    // Set to true if the sender has authority over the entity (is allowed to send replication updates for it).
    //
    // It is possible to have an entity with the Replicate component, but without authority.
    // For example:
    // - C1 replicates E to ClientOf C1' on the server
    // - on the server app, C1' does not have authority over the entity
    // - Replicate can be added on the entity in the server app to propagate replication updates to other clients
    //
    // If None, then the authority state is unknown.
    pub authority: Option<bool>,
}

impl PerSenderReplicationState {
    pub(crate) fn new(authority: Option<bool>) -> Self {
        Self { authority }
    }
    pub(crate) fn with_authority() -> Self {
        Self::new(Some(true))
    }
    pub(crate) fn without_authority() -> Self {
        Self::new(Some(false))
    }
}

impl Default for PerSenderReplicationState {
    fn default() -> Self {
        Self::new(None)
    }
}

mod private {
    pub trait Sealed {}
    impl Sealed for () {}
    #[cfg(feature = "prediction")]
    impl Sealed for lightyear_core::prediction::Predicted {}
    #[cfg(feature = "interpolation")]
    impl Sealed for lightyear_core::interpolation::Interpolated {}
}

#[doc(hidden)]
pub trait ReplicationTargetT: private::Sealed + Send + Sync + 'static {
    type VisibilityBit: Resource + Deref<Target = FilterBit>;
    type Context: Default;

    fn pre_insert(world: &mut DeferredWorld, entity: Entity);
    fn post_insert(context: &Self::Context, entity_mut: &mut EntityWorldMut);
    fn update_replicate_state(
        context: &mut Self::Context,
        state: &mut ReplicationState,
        sender_entity: Entity,
        host_client: bool,
    );

    fn on_replace(world: DeferredWorld, context: HookContext);
}

/// Marker component that indicates that the entity was replicated
/// from a remote world.
///
/// The component only exists while the peer does not have authority over
/// the entity.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct ReplicatedFrom {
    /// Entity that holds the [`ReplicationReceiver`](crate::receive::ReplicationReceiver) for this entity
    pub receiver: Entity,
}

impl ReplicationTargetT for () {
    type VisibilityBit = ReplicateBit;
    // Context = (the host-sender entity, does the current app have authority)
    type Context = (Option<Entity>, bool);

    fn pre_insert(world: &mut DeferredWorld, entity: Entity) {
        // update the authority broker if the entity is spawned on the server
        if let Some(peer_metadata) = world.get_resource::<PeerMetadata>()
            && let Some(server) = peer_metadata.mapping.get(&PeerId::Server)
            && let Some(mut broker) = world.get_mut::<AuthorityBroker>(*server)
        {
            // only set the authority if it didn't have an owner already (in case the authority was replicated
            // by another peer)
            broker.owners.entry(entity).or_insert(Some(PeerId::Server));
        }
    }
    fn post_insert(context: &Self::Context, entity_mut: &mut EntityWorldMut) {
        if context.1 {
            entity_mut.insert(HasAuthority);
        }
        if let Some(host_sender) = context.0 {
            entity_mut.insert((
                ReplicatedFrom {
                    receiver: host_sender,
                },
                // TODO: do we still need InitialReplicated?
                // SpawnedOnHostServer,
            ));
        }
    }

    fn update_replicate_state(
        context: &mut Self::Context,
        state: &mut ReplicationState,
        sender_entity: Entity,
        host_client: bool,
    ) {
        if host_client {
            context.0 = Some(sender_entity);
        }
        let remote_authoritative = state
            .per_sender_state
            .values()
            .any(|sender_state| sender_state.authority == Some(false));
        // only insert a sender if it was not already present
        // since it could already be present with no_authority (if we received the entity from a remote peer)
        state
            .per_sender_state
            .entry(sender_entity)
            .and_modify(|s| {
                // authority could be set to None (for example if PredictionTarget is processed first)
                if s.authority.is_none() && !remote_authoritative {
                    context.1 = true;
                }
            })
            .or_insert_with(|| {
                if remote_authoritative {
                    PerSenderReplicationState::without_authority()
                } else {
                    context.1 = true;
                    PerSenderReplicationState::with_authority()
                }
            });
    }

    fn on_replace(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let Ok(entity_ref) = world.get_entity(context.entity) else {
                return;
            };
            // Only remove Replicated if the Replicate component is actually being removed,
            // not just replaced with a new value
            let is_replacement = entity_ref.contains::<Replicate>();
            if !is_replacement {
                world.entity_mut(context.entity).remove::<Replicated>();
            }
            let Some(visibility_bit) = world.get_resource::<ReplicateBit>().map(|bit| bit.0) else {
                warn!(
                    entity = ?context.entity,
                    "Skipping replication target replacement because the visibility resource is missing"
                );
                return;
            };
            // TODO: after `DeferredWorld::as_unsafe_world_cell` becomes pub, put that outside of commands
            let unsafe_world = world.as_unsafe_world_cell();
            // SAFETY: we fetch data from distinct entities so there is no aliasing
            if let Some(state) =
                unsafe { unsafe_world.world() }.get::<ReplicationState>(context.entity)
            {
                state.per_sender_state.keys().for_each(|sender_entity| {
                    if let Some(mut visibility) = unsafe { unsafe_world.world_mut() }
                        .get_mut::<ClientVisibility>(*sender_entity)
                    {
                        visibility.set(context.entity, visibility_bit, false);
                    }
                });
            }
        });
    }
}

/// Entity-level visibility for [`Replicate`]
#[doc(hidden)]
#[derive(Resource, Deref)]
pub struct ReplicateBit(FilterBit);

impl FromWorld for ReplicateBit {
    fn from_world(world: &mut World) -> Self {
        let bit = world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
            world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                filter_registry.register_scope::<Entity>(world, &mut registry)
            })
        });
        Self(bit)
    }
}

#[cfg(feature = "prediction")]
mod prediction {
    use super::*;
    pub(super) use lightyear_core::prediction::Predicted;

    pub type PredictionTarget = ReplicationTarget<Predicted>;
    impl ReplicationTargetT for Predicted {
        type VisibilityBit = PredictedBit;

        // Context = the host-sender entity
        type Context = bool;

        fn pre_insert(_: &mut DeferredWorld, _: Entity) {}
        fn post_insert(context: &Self::Context, entity_mut: &mut EntityWorldMut) {
            if *context {
                entity_mut.insert(Self);
            }
        }

        fn update_replicate_state(
            context: &mut Self::Context,
            state: &mut ReplicationState,
            sender_entity: Entity,
            host_client: bool,
        ) {
            *context = host_client;
        }

        fn on_replace(mut world: DeferredWorld, context: HookContext) {
            let visibility_bit = *world.resource::<PredictedBit>().deref();
            world.commands().queue(move |world: &mut World| {
                if world.get_entity(context.entity).is_err() {
                    return;
                }
                let unsafe_world = world.as_unsafe_world_cell();
                // SAFETY: we fetch data from distinct entities so there is no aliasing
                if let Some(state) =
                    unsafe { unsafe_world.world() }.get::<ReplicationState>(context.entity)
                {
                    state.per_sender_state.keys().for_each(|sender_entity| {
                        if let Some(mut visibility) = unsafe { unsafe_world.world_mut() }
                            .get_mut::<ClientVisibility>(*sender_entity)
                        {
                            visibility.set(context.entity, visibility_bit, false);
                        }
                    });
                }
            });
        }
    }

    /// Component-level visibility for [`PredictionTarget`]
    #[doc(hidden)]
    #[derive(Resource, Deref)]
    pub struct PredictedBit(FilterBit);

    impl FromWorld for PredictedBit {
        fn from_world(world: &mut World) -> Self {
            let bit = world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
                world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    filter_registry
                        .register_scope::<SingleComponent<Predicted>>(world, &mut registry)
                })
            });
            Self(bit)
        }
    }
}

#[cfg(feature = "interpolation")]
mod interpolation {
    use super::*;
    pub(super) use lightyear_core::interpolation::Interpolated;

    pub type InterpolationTarget = ReplicationTarget<Interpolated>;
    impl ReplicationTargetT for Interpolated {
        type VisibilityBit = InterpolatedBit;
        // Context = the host-sender entity
        type Context = bool;

        fn pre_insert(_: &mut DeferredWorld, _: Entity) {}
        fn post_insert(context: &Self::Context, entity_mut: &mut EntityWorldMut) {
            if *context {
                entity_mut.insert(Self);
            }
        }

        fn update_replicate_state(
            context: &mut Self::Context,
            state: &mut ReplicationState,
            sender_entity: Entity,
            host_client: bool,
        ) {
            *context = host_client;
        }

        fn on_replace(mut world: DeferredWorld, context: HookContext) {
            let visibility_bit = *world.resource::<InterpolatedBit>().deref();
            world.commands().queue(move |world: &mut World| {
                if world.get_entity(context.entity).is_err() {
                    return;
                }
                let unsafe_world = world.as_unsafe_world_cell();
                // SAFETY: we fetch data from distinct entities so there is no aliasing
                if let Some(state) =
                    unsafe { unsafe_world.world() }.get::<ReplicationState>(context.entity)
                {
                    state.per_sender_state.keys().for_each(|sender_entity| {
                        if let Some(mut visibility) = unsafe { unsafe_world.world_mut() }
                            .get_mut::<ClientVisibility>(*sender_entity)
                        {
                            visibility.set(context.entity, visibility_bit, false);
                        }
                    });
                }
            });
        }
    }

    /// Component-level visibility for [`InterpolationTarget`]
    #[doc(hidden)]
    #[derive(Resource, Deref)]
    pub struct InterpolatedBit(FilterBit);

    impl FromWorld for InterpolatedBit {
        fn from_world(world: &mut World) -> Self {
            let bit = world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
                world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    filter_registry
                        .register_scope::<SingleComponent<Interpolated>>(world, &mut registry)
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

        T::pre_insert(&mut world, entity);

        // TODO: in 0.18 we don't need to put this in a command
        world.commands().queue(move |world: &mut World| {
            let mut context = T::Context::default();

            let unsafe_world = world.as_unsafe_world_cell();
            // SAFETY: we will use this world to access the ReplicationSender, and the other unsafe_world to access the entity
            let world = unsafe { unsafe_world.world_mut() };
            // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
            // SAFETY: there is no aliasing because the `entity_mut_state` is used to get these 4 components
            //  and `entity_mut` is used to insert some extra components
            let mut entity_mut = unsafe { unsafe_world.world_mut().entity_mut(entity) };
            let Ok((mut state, replicate)) = (unsafe {
                entity_mut.get_components_mut_unchecked::<(&mut ReplicationState, &Self)>
                ()
            }) else {
                return
            };

            match &replicate.mode {
                ReplicationMode::SingleSender => {
                    let Ok((sender_entity, mut visibility, host_client)) = world
                        .query_filtered::<(Entity, &mut ClientVisibility, Has<HostClient>), Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .single_mut(world)
                    else {
                        return;
                    };

                    T::update_replicate_state(&mut context, state.as_mut(), sender_entity, host_client);
                    visibility.set(entity, visibility_bit, true);
                }
                #[cfg(feature = "client")]
                ReplicationMode::SingleClient => {
                    use bevy_replicon::prelude::ConnectedClient;

                    let (sender_entity, host_client) =
                        if let Ok((sender_entity, mut visibility)) = world
                            .query_filtered::<(Entity, &mut ClientVisibility), With<HostClient>>()
                            .single_mut(world)
                        {
                            visibility.set(entity, visibility_bit, true);
                            (sender_entity, true)
                        } else if let Ok((sender_entity, mut visibility)) = world
                            .query_filtered::<
                                (Entity, &mut ClientVisibility),
                                With<ConnectedClient>,
                            >()
                            .single_mut(world)
                        {
                            visibility.set(entity, visibility_bit, true);
                            (sender_entity, false)
                        } else {
                            return;
                        };

                    if host_client {
                        let mut endpoints = world
                            .query_filtered::<&mut ClientVisibility, With<ConnectedClient>>();
                        for mut visibility in endpoints.iter_mut(world) {
                            visibility.set(entity, visibility_bit, false);
                        }
                    }

                    T::update_replicate_state(&mut context, state.as_mut(), sender_entity, host_client);
                }
                #[cfg(feature = "server")]
                ReplicationMode::SingleServer(target) => {
                    use lightyear_connection::client_of::ClientOf;
                    use lightyear_connection::host::HostClient;
                    use lightyear_connection::server::Started;
                    use lightyear_link::server::Server;
                    use tracing::{debug};
                    let unsafe_world = world.as_unsafe_world_cell();
                    // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
                    let world = unsafe { unsafe_world.world_mut() };
                    let Ok(server) = world
                        .query_filtered::<&Server, With<Started>>()
                        .single(world)
                    else {
                        debug!("Replicated before server actually existed, dont worry this case scenario is handled!");
                        return;
                    };
                    // SAFETY: we will use this to access the PeerMetadata, which does not alias with the ReplicationSenders
                    let peer_metadata = unsafe { unsafe_world.world() }
                        .resource::<PeerMetadata>();
                    let world = unsafe { unsafe_world.world_mut() };
                    let all_clients: alloc::vec::Vec<Entity> = server.collection().iter().copied().collect();
                    trace!(?entity, ?visibility_bit, num_clients = all_clients.len(), ?target, "SingleServer on_insert: setting visibility");
                    // First pass: hide for all clients (default is visible, so we must
                    // explicitly hide for non-target clients)
                    for &sender_entity in &all_clients {
                        if let Ok((mut visibility, _)) = world
                            .query_filtered::<(&mut ClientVisibility, Has<HostClient>),
                                (With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>)
                            >()
                            .get_mut(world, sender_entity)
                        {
                            trace!(?entity, ?sender_entity, "  hiding bit for client");
                            visibility.set(entity, visibility_bit, false);
                        }
                    }
                    // Second pass: show for target clients and update replicate state
                    target.apply_targets(
                        all_clients.into_iter(),
                        &peer_metadata.mapping,
                        &mut |sender_entity: Entity| {
                            let Ok((mut visibility, host_client)) = world
                                .query_filtered::<(&mut ClientVisibility, Has<HostClient>),
                                    (With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>)
                                >()
                                .get_mut(world, sender_entity)
                            else {
                                return;
                            };
                            trace!(?entity, ?sender_entity, "  showing bit for target client");
                            T::update_replicate_state(&mut context, state.as_mut(), sender_entity, host_client);
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
                    T::update_replicate_state(&mut context, state.as_mut(), sender_entity, host_client);
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
                        .query_filtered::<Entity, Or<(With<ReplicationSender>, With<HostClient>)>>(
                        )
                        .iter(world)
                        .collect();
                    for sender_entity in all_senders {
                        if let Ok(mut visibility) = world
                            .query_filtered::<&mut ClientVisibility, Or<(With<ReplicationSender>, With<HostClient>)>>()
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
                        T::update_replicate_state(&mut context, state.as_mut(), sender_entity, host_client);
                        visibility.set(entity, visibility_bit, true);
                    }
                }
            }

            T::post_insert(&context, &mut entity_mut);
        });
    }

    fn on_replace(world: DeferredWorld, context: HookContext) {
        T::on_replace(world, context)
    }
}

pub type ReplicationSendSystems = ServerSystems;

/// Replication is triggered in Replicon every time the `ServerTick` is incremented, which happens every
/// time the `Timer` in `ReplicationMetadata` finishes.
fn update_replication_tick(
    time: Res<Time>,
    timeline: Res<LocalTimeline>,
    mut replication_metadata: ResMut<ReplicationMetadata>,
    mut replication_tick: ResMut<ServerTick>,
) {
    replication_metadata.timer.tick(time.delta());
    if replication_metadata.timer.just_finished() {
        // Replicon requires a fresh tick for each replication send. Lightyear's
        // fixed simulation tick can stay unchanged between render frames, but we
        // still need a unique replication tick to flush spawns/updates that occur
        // between fixed ticks without reusing the previous mutate-confirm tick.
        let current_tick = replication_tick.get() as u16;
        let new_tick = timeline.tick();
        let delta = (new_tick - current_tick).0 as u32;
        replication_tick.increment_by(delta.max(1));
    }
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

    #[test]
    fn zero_timeline_delta_triggers_send_without_advancing_tick() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.insert_resource(LocalTimeline::default());
        app.insert_resource(ReplicationMetadata::default());
        app.init_resource::<ServerTick>();
        app.init_resource::<SendCount>();
        app.add_systems(Update, update_replication_tick);
        app.add_systems(
            Update,
            count_sends
                .after(update_replication_tick)
                .run_if(resource_changed::<ServerTick>),
        );
        app.world_mut().clear_trackers();

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(1));
        app.update();

        assert_eq!(app.world().resource::<ServerTick>().get(), 1);
        assert_eq!(app.world().resource::<SendCount>().0, 1);
    }

    #[test]
    fn timeline_advance_triggers_single_send() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.insert_resource(LocalTimeline::default());
        app.insert_resource(ReplicationMetadata::default());
        app.init_resource::<ServerTick>();
        app.init_resource::<SendCount>();
        app.add_systems(Update, update_replication_tick);
        app.add_systems(
            Update,
            count_sends
                .after(update_replication_tick)
                .run_if(resource_changed::<ServerTick>),
        );
        app.world_mut().clear_trackers();

        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(1);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(1));
        app.update();

        assert_eq!(app.world().resource::<ServerTick>().get(), 1);
        assert_eq!(app.world().resource::<SendCount>().0, 1);

        app.world_mut().clear_trackers();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_millis(1));
        app.update();

        assert_eq!(app.world().resource::<ServerTick>().get(), 2);
        assert_eq!(app.world().resource::<SendCount>().0, 2);
    }
}

pub struct SendPlugin;

/// When a new client gets `ClientVisibility`, set correct visibility bits
/// for all existing `PredictionTarget`/`InterpolationTarget` entities.
/// Without this, late-joining clients would see all components (including
/// Predicted/Interpolated markers that shouldn't be visible to them).
#[cfg(feature = "server")]
pub(crate) fn handle_new_client_visibility(
    trigger: On<Add, ClientVisibility>,
    remote_id_query: Query<&lightyear_core::id::RemoteId>,
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

    #[cfg(feature = "prediction")]
    for (entity, target) in prediction_targets.iter() {
        if let ReplicationMode::SingleServer(ref net_target) = target.mode {
            if !net_target.targets(&peer_id) {
                trace!(
                    ?entity,
                    ?sender_entity,
                    ?peer_id,
                    "  hiding predicted bit for non-target client"
                );
                visibility.set(entity, **predicted_bit, false);
            }
        }
    }

    #[cfg(feature = "interpolation")]
    for (entity, target) in interpolation_targets.iter() {
        if let ReplicationMode::SingleServer(ref net_target) = target.mode {
            if !net_target.targets(&peer_id) {
                trace!(
                    ?entity,
                    ?sender_entity,
                    ?peer_id,
                    "  hiding interpolated bit for non-target client"
                );
                visibility.set(entity, **interpolated_bit, false);
            }
        }
    }

    // Hide Controlled for entities not owned by this client
    for (entity, controlled_by) in controlled_entities.iter() {
        if controlled_by.owner != sender_entity {
            trace!(
                ?entity,
                ?sender_entity,
                "  hiding controlled bit for non-owner client"
            );
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
            PostUpdate,
            update_replication_tick.in_set(ServerSystems::IncrementTick),
        );

        #[cfg(any(feature = "prediction", feature = "interpolation"))]
        // We enable this replicon setting that does a few things:
        // - replication mutations are sent every RepliconTick, even if there were 0 mutations. This is to avoid
        //   situations where a client mispredicted something, and there is no correct since the sender didn't send
        //   any further updates
        // - it adds a `ServerMutateTicks` resource on the receiver that keeps track of the ticks where the receiver
        //   received any messages
        app.track_mutate_messages();

        // make sure that any ordering relative to ReplicationSystems is also applied to ServerSystems
        app.configure_sets(
            PostUpdate,
            ServerSystems::Send.in_set(ReplicationSystems::Send),
        );

        app.register_required_components::<Replicate, Replicated>();
        app.init_resource::<ReplicateBit>();
        app.init_resource::<VisibilityBit>();
        #[cfg(feature = "prediction")]
        {
            app.register_required_components::<PredictionTarget, Predicted>();
            // Note: app.replicate::<Predicted>() is called in SharedComponentRegistrationPlugin
            // to ensure matching component IDs on both client and server.
            app.init_resource::<PredictedBit>();
        }
        #[cfg(feature = "interpolation")]
        {
            app.register_required_components::<InterpolationTarget, Interpolated>();
            // Note: app.replicate::<Interpolated>() is called in SharedComponentRegistrationPlugin
            app.init_resource::<InterpolatedBit>();
        }
    }
}
