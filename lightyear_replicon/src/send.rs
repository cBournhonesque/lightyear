//! Handles visibility rules for Replicate, PredictionTarget, and InterpolationTarget components.
use alloc::vec::Vec;
use core::ops::Deref;
use bevy_app::prelude::*;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::world::DeferredWorld;
use bevy_derive::Deref;
use bevy_ecs::entity::EntityIndexMap;
use bevy_reflect::Reflect;
#[allow(unused_imports)]
use bevy_replicon::prelude::{ComponentScope, FilterScope, Replicated, VisibilityFilter};
use bevy_replicon::server::visibility::client_visibility::ClientVisibility;
use bevy_replicon::server::visibility::filters_mask::FilterBit;
use bevy_replicon::server::visibility::registry::FilterRegistry;
use bevy_replicon::shared::replication::registry::ReplicationRegistry;
use serde::{Deserialize, Serialize};
use lightyear_connection::client::{Client, PeerMetadata};
use lightyear_connection::host::HostClient;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::id::PeerId;
use crate::authority::{AuthorityBroker, HasAuthority};

#[allow(unused_imports)]
use tracing::{error, trace};

#[cfg(feature = "prediction")]
pub use prediction::*;
#[cfg(feature = "interpolation")]
pub use interpolation::*;

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
#[derive(Component)]
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
    pub fn state(&self) -> &EntityIndexMap<lightyear_replication::prelude::PerSenderReplicationState> {
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
        Self {
            #[cfg(feature = "prediction")]
            predicted: false,
            #[cfg(feature = "interpolation")]
            interpolated: false,
            authority,
        }
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

#[doc(hidden)]
pub trait ReplicationTargetT: Send + Sync + 'static {
    type VisibilityBit: Resource + Deref<Target=FilterBit>;
    type Context: Default;

    fn pre_insert(world: &mut DeferredWorld, entity: Entity);
    fn post_insert(context: &Self::Context, entity_mut: &mut EntityWorldMut);
    fn update_replicate_state(context: &mut Self::Context, state: &mut ReplicationState, sender_entity: Entity, host_client: bool);
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
        if let Some(peer_metadata) = world.get_resource::<PeerMetadata>() && let Some(server) = peer_metadata.mapping.get(&PeerId::Server) && let Some(mut broker) = world.get_mut::<AuthorityBroker>(*server) {
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
                ReplicatedFrom { receiver: host_sender },
                // TODO: do we still need InitialReplicated?
                // SpawnedOnHostServer,
            ));
        }
    }

    fn update_replicate_state(context: &mut Self::Context, state: &mut ReplicationState, sender_entity: Entity, host_client: bool) {
        if host_client {
            context.0 = Some(sender_entity);
        }
        // only insert a sender if it was not already present
        // since it could already be present with no_authority (if we received the entity from a remote peer)
        state.per_sender_state.entry(sender_entity)
            .and_modify(|s| {
                // authority could be set to None (for example if PredictionTarget is processed first)
                if s.authority.is_none() {
                    context.1 = true;
                }
            })
            .or_insert_with(|| {
                context.1 = true;
                PerSenderReplicationState::with_authority()
            });
    }
}


/// Entity-level visibility for [`Replicate`]
#[derive(Resource, Deref)]
struct ReplicateBit(FilterBit);

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
    use lightyear_core::prediction::Predicted;

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

        fn update_replicate_state(context: &mut Self::Context, state: &mut ReplicationState, sender_entity: Entity, host_client: bool) {
            *context = host_client;
        }
    }

    /// Component-level visibility for [`PredictionTarget`]
    #[derive(Resource, Deref)]
    struct PredictedBit(FilterBit);

    impl FromWorld for PredictedBit {
        fn from_world(world: &mut World) -> Self {
            let bit = world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
                world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    filter_registry.register_scope::<ComponentScope<Predicted>>(world, &mut registry)
                })
            });
            Self(bit)
        }
    }

    impl PredictionTarget {
        fn on_replace(mut world: DeferredWorld, context: HookContext) {
            let visibility_bit = *world.resource::<PredictedBit>().deref();
            let unsafe_world = world.as_unsafe_world_cell();
            // SAFETY: we fetch data from distinct entities so there is no aliasing
            if let Some(state) = unsafe { unsafe_world.world() }.get::<ReplicationState>(context.entity) {
                state.per_sender_state.keys().for_each(|sender_entity| {
                    if let Some(mut visibility) = unsafe{ unsafe_world.world_mut() }.get_mut::<ClientVisibility>(*sender_entity) {
                        visibility.set(context.entity, visibility_bit, false);
                    }
                });
            }
        }
    }
}


#[cfg(feature = "interpolation")]
mod interpolation {
    use super::*;
    use lightyear_core::interpolation::Interpolated;

    pub type InterpolationTarget = ReplicationTarget<Interpolated>;
    impl ReplicationTargetT for Interpolated {
        type VisibilityBit = ReplicateBit;
        // Context = the host-sender entity
        type Context = bool;

        fn pre_insert(_: &mut DeferredWorld, _: Entity) {}
        fn post_insert(context: &Self::Context, entity_mut: &mut EntityWorldMut) {
            if *context {
                entity_mut.insert(Self);
            }
        }

        fn update_replicate_state(context: &mut Self::Context, state: &mut ReplicationState, sender_entity: Entity, host_client: bool) {
            *context = host_client;
        }
    }


    /// Component-level visibility for [`InterpolatedTarget`]
    #[derive(Resource, Deref)]
    struct InterpolatedBit(FilterBit);

    impl FromWorld for InterpolatedBit {
        fn from_world(world: &mut World) -> Self {
            let bit = world.resource_scope(|world, mut filter_registry: Mut<FilterRegistry>| {
                world.resource_scope(|world, mut registry: Mut<ReplicationRegistry>| {
                    filter_registry.register_scope::<ComponentScope<Interpolated>>(world, &mut registry)
                })
            });
            Self(bit)
        }
    }

    impl InterpolationTarget {
        fn on_replace(mut world: DeferredWorld, context: HookContext) {
            let visibility_bit = *world.resource::<InterpolatedBit>().deref();
            let unsafe_world = world.as_unsafe_world_cell();
            // SAFETY: we fetch data from distinct entities so there is no aliasing
            if let Some(state) = unsafe { unsafe_world.world() }.get::<ReplicationState>(context.entity) {
                state.per_sender_state.keys().for_each(|sender_entity| {
                    if let Some(mut visibility) = unsafe{ unsafe_world.world_mut() }.get_mut::<ClientVisibility>(*sender_entity) {
                        visibility.set(context.entity, visibility_bit, false);
                    }
                });
            }
        }
    }
}


impl<T: ReplicationTargetT> ReplicationTarget<T> {
    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        let entity = context.entity;
        let visibility_bit = *world.resource::<T::VisibilityBit>().deref();

        T::pre_insert(&mut world, entity);

        let mut context = T::Context::default();

        let unsafe_world = world.as_unsafe_world_cell();
        // SAFETY: we will use this world to access the ReplicationSender, and the other unsafe_world to access the entity
        let world = unsafe { unsafe_world.world_mut() };
        // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
        // SAFETY: there is no aliasing because the `entity_mut_state` is used to get these 4 components
        //  and `entity_mut` is used to insert some extra components
        let mut entity_mut = unsafe { unsafe_world.world_mut().entity_mut(entity) };
        let Some((mut state, replicate)) = (unsafe {
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
            ReplicationMode::SingleClient => {
                let Ok((sender_entity, mut visibility, host_client)) = world
                        .query_filtered::<
                            (Entity, &mut ClientVisibility, Has<HostClient>),
                            (With<Client>, Or<(With<ReplicationSender>, With<HostClient>)>)
                        >()
                        .single_mut(world)
                    else {
                        return;
                    };
                T::update_replicate_state(&mut context, state.as_mut(), sender_entity, host_client);
                visibility.set(entity, visibility_bit, true);
            }
            #[cfg(feature = "server")]
            ReplicationMode::SingleServer(target) => {
                    use lightyear_connection::client_of::ClientOf;
                    use lightyear_connection::host::HostClient;
                    use lightyear_connection::server::Started;
                    use lightyear_link::server::Server;
                    use tracing::{debug, error};
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
                    target.apply_targets(
                        server.collection().iter().copied(),
                        &peer_metadata.mapping,
                        &mut |sender_entity| {
                            let Ok((mut visibility, host_client)) = world
                                .query_filtered::<(&mut ClientVisibility, Has<HostClient>),
                                    (With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>)
                                >()
                                .get_mut(world, sender_entity)
                            else {
                                return;
                            };
                           T::update_replicate_state(&mut context, state.as_mut(), sender_entity, host_client);
                    visibility.set(entity, visibility_bit, true);
                        },
                    );

            }
            ReplicationMode::Sender(sender_entity) => {
                 let Ok((mut visibility, host_client)) = world
                        .query_filtered::<(&mut ClientVisibility, Has<HostClient>), Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .get_mut(world, *sender_entity)
                else {
                    return;
                };
                T::update_replicate_state(&mut context, state.as_mut(), *sender_entity, host_client);
                visibility.set(entity, visibility_bit, true);
            }
            #[cfg(feature = "server")]
            ReplicationMode::Server(_, _) => {
                unimplemented!()
            }
            ReplicationMode::Target(_) => {
                unimplemented!()
            }
            ReplicationMode::Manual(_) => {
                unimplemented!()
            }
        }

        T::post_insert(&context, &mut entity_mut);
    }
}


impl ReplicationTarget<()> {
    fn on_replace(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            world.entity_mut(context.entity).remove::<Replicated>();
        });
        let visibility_bit = world.resource::<ReplicateBit>().0;
        // TODO: after `DeferredWorld::as_unsafe_world_cell` becomes pub, put that outside of commands
        let unsafe_world = world.as_unsafe_world_cell();
        // SAFETY: we fetch data from distinct entities so there is no aliasing
        if let Some(state) = unsafe { unsafe_world.world() }.get::<ReplicationState>(context.entity) {
            state.per_sender_state.keys().for_each(|sender_entity| {
                if let Some(mut visibility) = unsafe{ unsafe_world.world_mut() }.get_mut::<ClientVisibility>(*sender_entity) {
                    visibility.set(context.entity, visibility_bit, false);
                }
            });
        }
    }
}


pub struct SendPlugin;
impl Plugin for SendPlugin{
    fn build(&self, app: &mut App) {
        app.register_required_components::<Replicate, Replicated>();
        app.init_resource::<ReplicateBit>();
        #[cfg(feature = "prediction")]
        {
            app.register_required_components::<PredictionTarget, Predicted>();
            app.init_resource::<PredictedBit>();
        }
        #[cfg(feature = "interpolation")]
        {
            use prediction::*;
            app.register_required_components::<InterpolationTarget, Interpolated>();
            app.init_resource::<InterpolatedBit>();
        }
    }
}


