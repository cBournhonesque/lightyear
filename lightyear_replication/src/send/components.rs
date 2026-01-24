use crate::authority::{AuthorityBroker, HasAuthority};
use crate::components::{ComponentReplicationOverrides, InitialReplicated, Replicated};
use crate::send::sender::ReplicationSender;
use alloc::vec::Vec;
use bevy_ecs::component::Component;
use bevy_ecs::entity::EntityIndexMap;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::query::QueryData;
use bevy_ecs::reflect::ReflectComponent;
use bevy_ecs::world::DeferredWorld;
use bevy_reflect::Reflect;
use bevy_time::{Timer, TimerMode};
use lightyear_connection::client::{Client, Connected, PeerMetadata};
use lightyear_connection::host::HostClient;
use lightyear_connection::network_target::NetworkTarget;
#[cfg(feature = "server")]
use {
    bevy_utils::prelude::DebugName, lightyear_connection::client_of::ClientOf,
    lightyear_link::server::Server,
};

use crate::host::SpawnedOnHostServer;
use crate::send::plugin::ReplicableRootEntities;
use crate::visibility::immediate::VisibilityState;
use lightyear_core::id::{PeerId, RemoteId};
use lightyear_link::server::LinkOf;
use lightyear_serde::entity_map::SendEntityMap;
use lightyear_serde::prelude::{Seek, SeekFrom};
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use tracing::{debug, error, info, trace};

/// Default replication group which corresponds to the absence of a group:
/// updates will be packed into a single message up to the MTU
/// but there is no guarantee that updates for entities in this group are sent
/// together
pub const DEFAULT_GROUP: ReplicationGroup = ReplicationGroup::new_id(0);
/// Replication group shared by all predicted entities
pub const PREDICTION_GROUP: ReplicationGroup = ReplicationGroup::new_id(1);

#[derive(Debug, Default, PartialEq, Clone, Reflect)]
pub struct ComponentReplicationConfig {
    /// by default we will replicate every update for the component. If this is True, we will only
    /// replicate the inserts/removes of the component.
    pub replicate_once: bool,
    /// by default, a component in the registry will get replicated when added to a Replicated entity
    /// If true, the default behaviour is flipped. The component is not replicated by default and has
    /// to be explicitly enabled.
    pub disable: bool,
    /// If true, the component will be replicated using delta compression
    pub delta_compression: bool,
}

#[derive(Debug, Default, Reflect)]
pub struct ComponentReplicationOverride {
    pub disable: bool,
    pub enable: bool,
    pub replicate_once: bool,
    pub replicate_always: bool,
    pub entity_map: SendEntityMap,
}

impl<C> ComponentReplicationOverrides<C> {
    /// Get component overrides for a specific sender
    pub fn get_overrides(&self, sender: Entity) -> Option<&ComponentReplicationOverride> {
        if let Some(overrides) = self.per_sender.get(&sender) {
            return Some(overrides);
        }
        self.all_senders.as_ref()
    }

    /// Returns true if the component is disabled for all senders
    pub fn is_disabled_for_all(&self, mut registry_disable: bool) -> bool {
        let disable = &mut registry_disable;
        if let Some(all) = &self.all_senders {
            if all.disable {
                *disable = true;
            }
            if all.enable {
                *disable = false;
            }
        }
        // if all_senders is disabled, we only return true if no per_sender overrides are enabled
        *disable && !self.per_sender.values().any(|o| o.enable)

        // TODO: there is the edge case where all the senders have enabled the component!
    }

    /// Add an override for all senders
    pub fn global_override(&mut self, overrides: ComponentReplicationOverride) {
        self.all_senders = Some(overrides);
    }

    /// Add an override for a specific sender. Takes priority over any global override
    pub fn override_for_sender(&mut self, overrides: ComponentReplicationOverride, sender: Entity) {
        self.per_sender.insert(sender, overrides);
    }

    pub fn disable_for(mut self, sender: Entity) -> Self {
        let o = self.per_sender.entry(sender).or_default();
        o.disable = true;
        o.enable = false;
        self
    }

    pub fn disable_all(mut self) -> Self {
        let o = self.all_senders.get_or_insert_default();
        o.disable = true;
        o.enable = false;
        self
    }

    pub fn enable_for(mut self, sender: Entity) -> Self {
        let o = self.per_sender.entry(sender).or_default();
        o.enable = true;
        o.disable = false;
        self
    }

    pub fn enable_all(mut self) -> Self {
        let o = self.all_senders.get_or_insert_default();
        o.enable = true;
        o.disable = false;
        self
    }

    pub fn replicate_once_for(mut self, sender: Entity) -> Self {
        let o = self.per_sender.entry(sender).or_default();
        o.replicate_once = true;
        o.replicate_always = false;
        self
    }

    pub fn replicate_once_all(mut self) -> Self {
        let o = self.all_senders.get_or_insert_default();
        o.replicate_once = true;
        o.replicate_always = false;
        self
    }
}

/// Marker component to indicate that updates for this entity are being replicated.
///
/// If this component gets removed, the replication will pause.
#[derive(Component, Clone, Copy, Default, PartialEq, Debug, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct Replicating;

/// Keeps track of the last known state of a component, so that we can compute
/// the delta between the old and new state.
#[derive(Component, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct Cached<C> {
    pub value: C,
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Reflect)]
pub enum ReplicationGroupIdBuilder {
    // the group id is the entity id
    #[default]
    FromEntity,
    // choose a different group id
    // note: it must not be the same as any entity id!
    // TODO: how can i generate one that doesn't conflict with an existing entity? maybe take u32 as input, and apply generation = u32::MAX - 1?
    //  or reserver some entities on the sender world?
    Group(u64),
}

/// Component to specify the replication group of an entity
///
/// If multiple entities are part of the same replication group, they will be sent together in the same message.
/// It is guaranteed that these entities will be updated at the same time on the remote world.
///
/// There is one exception: the default (ReplicationGroup(0)) which doesn't guarantee that all updates
/// will be sent in the same message.
#[derive(Component, Debug, Clone, PartialEq, Reflect)]
#[reflect(Component)]
pub struct ReplicationGroup {
    id_builder: ReplicationGroupIdBuilder,
    /// the priority of the accumulation group
    /// (priority will get reset to this value every time a message gets sent successfully)
    base_priority: f32,
    /// Keep track of whether we should send replication updates for this group.
    ///
    /// See [`ReplicationGroup::set_send_frequency`] for more information.
    pub send_frequency: Option<Timer>,
    /// Is true if we should send replication updates for this group.
    ///
    /// The interaction with `send_frequency` is as follows:
    /// Time:               0    10   20    30    40    50    60    70    80    90    100
    /// GroupTimer(30ms):   X               X                 X                 X
    /// SendInterval(20ms): X          X          X           X           X           X
    ///
    /// At 40ms, 60ms and 100ms, we will buffer the replication updates for the group.
    /// (We do not buffer the updates exactly at 30ms, 60ms, 90ms; instead we wait for the next send_interval.
    /// This is to avoid having to track the send_tick for each replication group separately)
    // TODO: maybe buffer the updates exactly at 30ms, 60ms, 90ms and include the send_tick in the message?
    pub should_send: bool,
}

impl Default for ReplicationGroup {
    fn default() -> Self {
        Self {
            id_builder: ReplicationGroupIdBuilder::Group(0),
            base_priority: 1.0,
            send_frequency: None,
            should_send: true,
        }
    }
}

impl ReplicationGroup {
    pub const fn new_from_entity() -> Self {
        Self {
            id_builder: ReplicationGroupIdBuilder::FromEntity,
            base_priority: 1.0,
            send_frequency: None,
            should_send: true,
        }
    }

    pub const fn new_id(id: u64) -> Self {
        Self {
            id_builder: ReplicationGroupIdBuilder::Group(id),
            base_priority: 1.0,
            send_frequency: None,
            should_send: true,
        }
    }

    #[inline]
    pub fn group_id(&self, entity: Option<Entity>) -> ReplicationGroupId {
        match self.id_builder {
            ReplicationGroupIdBuilder::FromEntity => {
                ReplicationGroupId(entity.expect("need to provide an entity").to_bits())
            }
            ReplicationGroupIdBuilder::Group(id) => ReplicationGroupId(id),
        }
    }

    pub fn priority(&self) -> f32 {
        self.base_priority
    }

    pub fn set_priority(mut self, priority: f32) -> Self {
        self.base_priority = priority;
        self
    }

    pub fn set_id(mut self, id: u64) -> Self {
        self.id_builder = ReplicationGroupIdBuilder::Group(id);
        self
    }

    /// Sets the send frequency for this [`ReplicationGroup`]
    ///
    /// Any replication updates related to this group will only be buffered at the specified frequency.
    /// It is INCORRECT to set the send_frequency to be more frequent than the sender's send_interval.
    ///
    /// This can be useful to send updates for a group of entities less frequently than the default send_interval.
    /// For example the send_interval could be 30Hz, but you could set the send_frequency to 10Hz for a group of entities
    /// to buffer updates less frequently.
    pub fn set_send_frequency(mut self, send_frequency: core::time::Duration) -> Self {
        self.send_frequency = Some(Timer::new(send_frequency, TimerMode::Repeating));
        self
    }
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub struct ReplicationGroupId(pub u64);

// Re-use the Entity serialization since ReplicationGroupId are often entities
impl ToBytes for ReplicationGroupId {
    fn bytes_len(&self) -> usize {
        if self.0 == 0 {
            1
        } else {
            Entity::from_bits(self.0).bytes_len()
        }
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        if self.0 == 0 {
            buffer.write_u8(0)?;
        } else {
            Entity::from_bits(self.0).to_bytes(buffer)?;
        }
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let first = buffer.read_u8()?;
        if first == 0 {
            return Ok(Self(0));
        }
        // go back one byte
        buffer.seek(SeekFrom::Current(-1))?;
        Ok(Self(Entity::from_bytes(buffer)?.to_bits()))
    }
}

pub type PredictionTarget = ReplicationTarget<lightyear_core::prediction::Predicted>;

#[cfg(feature = "prediction")]
impl PredictionTarget {
    pub fn add_replication_group(trigger: On<Add, PredictionTarget>, mut commands: Commands) {
        // note: we don't need to handle this for ReplicateLike entities because they take the ReplicationGroup from the root entity
        commands.entity(trigger.entity).insert(PREDICTION_GROUP);
    }
}

impl ReplicationTargetT for lightyear_core::prediction::Predicted {
    fn update_host_client(entity_mut: &mut EntityWorldMut) {
        entity_mut.insert(Self);
    }

    fn update_replicate_state(state: &mut PerSenderReplicationState) {
        #[cfg(feature = "prediction")]
        {
            state.predicted = true;
        }
    }

    fn clear_replicate_state(state: &mut PerSenderReplicationState) {
        #[cfg(feature = "prediction")]
        {
            state.predicted = false;
        }
    }
}

pub type InterpolationTarget = ReplicationTarget<lightyear_core::interpolation::Interpolated>;

impl ReplicationTargetT for lightyear_core::interpolation::Interpolated {
    fn update_host_client(entity_mut: &mut EntityWorldMut) {
        entity_mut.insert(Self);
    }

    fn update_replicate_state(state: &mut PerSenderReplicationState) {
        #[cfg(feature = "interpolation")]
        {
            state.interpolated = true;
        }
    }

    fn clear_replicate_state(state: &mut PerSenderReplicationState) {
        #[cfg(feature = "interpolation")]
        {
            state.interpolated = false;
        }
    }
}

/// Insert this component to specify which remote peers will start predicting the entity
/// upon receiving the entity.
// NOTE: we don't require Replicate here because we might be using this with ReplicateLike entities
//  in order to override the prediction/interpolation targets.
#[derive(Component, Clone, Default, Debug, PartialEq, Reflect)]
#[require(ReplicationState)]
#[component(on_insert = ReplicationTarget::<T>::on_insert)]
#[component(on_replace = ReplicationTarget::<T>::on_replace)]
pub struct ReplicationTarget<T: ReplicationTargetT> {
    mode: ReplicationMode,
    #[reflect(ignore)]
    marker: core::marker::PhantomData<T>,
}

#[doc(hidden)]
pub trait ReplicationTargetT: Send + Sync + 'static {
    fn update_host_client(entity_mut: &mut EntityWorldMut);
    fn update_replicate_state(state: &mut PerSenderReplicationState);

    fn clear_replicate_state(state: &mut PerSenderReplicationState);
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

    pub(crate) fn on_insert(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let unsafe_world = world.as_unsafe_world_cell();
            // SAFETY: we will use this world to access the ReplicationSender
            let world = unsafe { unsafe_world.world_mut() };
            // SAFETY: we will use this world only to access the Replicated entity, so there is no aliasing issue
            let mut entity_mut =
                unsafe { unsafe_world.world_mut().entity_mut(context.entity) };
            let mut entity_mut_state =
                unsafe { unsafe_world.world_mut().entity_mut(context.entity) };

            let (replicate, mut state) =
                // SAFETY: there is no aliasing because `entity_mut_state` just fetches ReplicationTarget and ReplicationState,
                //  and `entity_mut` inserts some unrelated components
                unsafe { entity_mut_state.get_components_mut_unchecked::<(&ReplicationTarget<T>, &mut ReplicationState)>() }
                .unwrap();

            // trackers to check if we need to insert extra components
            // (we cannot insert components on the entity while holding on to `&mut ReplicationState` as the entity would be moved
            // to another archetype
            let mut add_host_client = false;

            // enable split borrows
            match &replicate.mode {
                ReplicationMode::SingleSender => {
                    let Ok((sender_entity, is_host_client)) = world.query_filtered::<(Entity, Has<HostClient>), Or<(With<ReplicationSender>, With<HostClient>)>>().single_mut(world) else {
                        trace!(mode = ?replicate.mode, "No ReplicationSender found in the world");
                        return;
                    };
                    T::update_replicate_state(state.per_sender_state.entry(sender_entity).or_default());
                    if is_host_client {
                        add_host_client = true;
                    }
                }
                #[cfg(feature = "client")]
                ReplicationMode::SingleClient => {
                    use lightyear_connection::client::Client;
                    use lightyear_connection::host::HostClient;
                    let Ok((sender_entity, is_host_client)) = world
                        .query_filtered::<(Entity, Has<HostClient>), (With<Client>, Or<(With<ReplicationSender>, With<HostClient>)>)>()
                        .single_mut(world)
                    else {
                        return;
                    };
                    T::update_replicate_state(state.per_sender_state.entry(sender_entity).or_default());
                    if is_host_client {
                        add_host_client = true;
                    }
                }
                #[cfg(feature = "server")]
                ReplicationMode::SingleServer(target) => {
                    let unsafe_world = world.as_unsafe_world_cell();
                    // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
                    let server_world = unsafe { unsafe_world.world_mut() };
                    let Ok(server) = server_world
                        .query_filtered::<&Server, With<Server>>()
                        .single(server_world)
                    else {
                        debug!("Replicated before server actually existed, dont worry this case scenario is handled!");
                        return;
                    };
                    // SAFETY: we will use this to access the PeerMetadata, which does not alias with the ReplicationSenders
                    let peer_metadata = unsafe { unsafe_world.world() }.resource::<PeerMetadata>();
                    let world = unsafe { unsafe_world.world_mut() };
                    target.apply_targets(
                        server.collection().iter().copied(),
                        &peer_metadata.mapping,
                        &mut |client| {
                            trace!(
                                "Adding ReplicationTarget<{}>, entity {} to ClientOf {}",
                                DebugName::type_name::<T>(),
                                context.entity,
                                client
                            );
                            let Ok(is_host_client) = world
                                .query_filtered::<Has<HostClient>, (With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>)>()
                                .get_mut(world, client)
                            else {
                                return;
                            };
                            T::update_replicate_state(state.per_sender_state.entry(client).or_default());
                            if is_host_client {
                                add_host_client = true;
                            }
                        },
                    );
                }
                ReplicationMode::Sender(entity) => {
                    let Ok(is_host_client) = world
                        .query_filtered::<Has<HostClient>, Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .get_mut(world, *entity)
                    else {
                        return;
                    };
                    T::update_replicate_state(state.per_sender_state.entry(*entity).or_default());
                    if is_host_client {
                        add_host_client = true;
                    }
                }
                #[cfg(feature = "server")]
                ReplicationMode::Server(server, target) => {
                    let unsafe_world = world.as_unsafe_world_cell();
                    // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
                    let entity_ref = unsafe { unsafe_world.world() }.entity(*server);
                    if !entity_ref.contains::<Server>() {
                        debug!("Replicated before server actually existed, dont worry this case scenario is handled!");
                        return;
                    }
                    let Some(server) = entity_ref.get::<Server>() else {
                        debug!("Replicated before server actually existed, dont worry this case scenario is handled!");
                        return;
                    };
                    // SAFETY: we will use this to access the PeerMetadata, which does not alias with the ReplicationSenders
                    let peer_metadata = unsafe { unsafe_world.world() }.resource::<PeerMetadata>();
                    let world = unsafe { unsafe_world.world_mut() };
                    target.apply_targets(
                        server.collection().iter().copied(),
                        &peer_metadata.mapping,
                        &mut |client| {
                            let Ok(is_host_client) = world
                                .query_filtered::<Has<HostClient>, (With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>)>()
                                .get_mut(world, client)
                            else {
                                return;
                            };
                            T::update_replicate_state(state.per_sender_state.entry(client).or_default());
                            if is_host_client {
                                add_host_client = true;
                            }
                        },
                    );
                }
                ReplicationMode::Target(_) => {
                    todo!(
                        "need a global mapping from remote_peer to corresponding replication_sender"
                    )
                }
                ReplicationMode::Manual(sender_entities) => {
                    for sender_entity in sender_entities.iter() {
                        let Ok(is_host_client) = world
                            .query_filtered::<Has<HostClient>, Or<(With<ReplicationSender>, With<HostClient>)>>()
                            .get_mut(world, *sender_entity)
                        else {
                            return;
                        };
                        T::update_replicate_state(state.per_sender_state.entry(*sender_entity).or_default());
                        if is_host_client {
                            add_host_client = true;
                        }
                    }
                }
            }
            if add_host_client {
                T::update_host_client(&mut entity_mut);
            }
        });
    }

    pub(crate) fn on_replace(mut world: DeferredWorld, context: HookContext) {
        let mut state = world.get_mut::<ReplicationState>(context.entity).unwrap();
        state.per_sender_state.values_mut().for_each(|v| {
            T::clear_replicate_state(v);
        });
    }
}

// TODO: accept EntityTarget on top of NetworkTarget.
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

/// Component containins per-[`ReplicationSender`] metadata for the entity.
///
/// This can be used to update the visibility of the entity if [`NetworkVisibility`](crate::visibility::immediate::NetworkVisibility)
/// is present on the entity.
///
/// ```
/// # use bevy_ecs::prelude::*;
/// # use lightyear_replication::prelude::{NetworkVisibility, Replicate, ReplicationState};
/// # let mut world = World::new();
/// # let entity = world.spawn((ReplicationState::default(), NetworkVisibility)).id();
/// # let mut sender = world.spawn_empty().id();
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

    /// Returns `true` if the entity is visible for `sender`,
    /// and `false` if there is no [`NetworkVisibility`](crate::visibility::immediate::NetworkVisibility), no sender entry, or visibility is not `Visible`/`Gained`.
    pub fn is_visible(&self, sender: Entity) -> bool {
        self.per_sender_state.get(&sender).is_some_and(|s| {
            matches!(
                s.visibility,
                VisibilityState::Visible | VisibilityState::Gained
            )
        })
    }

    /// Indicate that the entity is not visible for that [`ReplicationSender`] entity.
    pub fn lose_visibility(&mut self, sender: Entity) {
        let state = self
            .per_sender_state
            .entry(sender)
            .or_insert_with(PerSenderReplicationState::with_authority);
        // if we just set it to Gained before we could tick the visibility, it cancels out
        if state.visibility == VisibilityState::Gained {
            state.visibility = VisibilityState::Default;
        } else if state.visibility != VisibilityState::Lost {
            state.visibility = VisibilityState::Lost;
        }
    }

    /// Indicate that the entity is now visible for that [`ReplicationSender`] entity.
    pub fn gain_visibility(&mut self, sender: Entity) {
        let state = self
            .per_sender_state
            .entry(sender)
            .or_insert_with(PerSenderReplicationState::with_authority);
        // if the entity was already relevant (Relevance::Maintained), be careful to not set it to
        // Relevance::Gained as it would trigger a duplicate spawn replication action
        if !matches!(
            state.visibility,
            VisibilityState::Visible | VisibilityState::Gained
        ) {
            state.visibility = VisibilityState::Gained;
        };
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

// TODO: maybe also add disabling/component-overrides here?
//  if any is enabled then we add a marker component to identify those archetypes
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Reflect)]
pub struct PerSenderReplicationState {
    #[cfg(feature = "prediction")]
    pub(crate) predicted: bool,
    #[cfg(feature = "interpolation")]
    pub(crate) interpolated: bool,
    pub(crate) visibility: VisibilityState,
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
    // TODO: maybe set this back to false on LoseVisibility? But what if we lose and gain it quickly
    //   before a replication update?
    // Set to true if the `ReplicationSender` sent a 'spawn' message for this entity
    //
    // This is needed because we cannot simply rely on seeing if `Replicate` has changed to check if we need to spawn the entity again.
    // For example:
    // - spawn Replicate to client 1
    // - spawn message is sent to client 1
    // - add client 2 as a replication target
    // -> Replicate has changed, but we only want to send a 'spawn' message to client 1
    pub(crate) spawned: bool,
    // Indicates that this sender data should be removed after a Replicate change.
    // For example if we were replicating to clients [1, 2] and we change to replicating to client [1].
    // OnReplace: set `to_remove=true` to [1, 2]
    // OnInsert: reset `to_remove=false` on [1], discard [2] since `to_remove=true`
    pub(crate) to_remove: bool,
}

impl PerSenderReplicationState {
    pub(crate) fn new(authority: Option<bool>) -> Self {
        Self {
            #[cfg(feature = "prediction")]
            predicted: false,
            #[cfg(feature = "interpolation")]
            interpolated: false,
            visibility: VisibilityState::default(),
            authority,
            spawned: false,
            to_remove: false,
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

/// Insert this component to start replicating your entity.
///
/// - If sender is an Entity that has a ReplicationSender, we will replicate on that entity
/// - If the entity is None, we will try to find a unique ReplicationSender in the app
#[derive(Component, Clone, Default, Debug, PartialEq, Reflect)]
#[require(Replicating)]
#[require(ReplicationGroup)]
#[require(ReplicationState)]
#[component(on_insert = Replicate::on_insert)]
#[component(on_replace = Replicate::on_replace)]
#[reflect(Component)]
pub struct Replicate {
    /// Defines which [`ReplicationSenders`](ReplicationSender) this entity will be replicated to
    mode: ReplicationMode,
}

impl Replicate {
    pub fn new(mode: ReplicationMode) -> Self {
        Self { mode }
    }

    #[cfg(feature = "client")]
    pub fn to_server() -> Self {
        Self {
            mode: ReplicationMode::SingleClient,
        }
    }

    #[cfg(feature = "server")]
    pub fn to_clients(target: NetworkTarget) -> Self {
        Self {
            mode: ReplicationMode::SingleServer(target),
        }
    }

    pub fn manual(senders: Vec<Entity>) -> Self {
        Self {
            mode: ReplicationMode::Manual(senders),
        }
    }

    // We NEVER manually update replicate, so we can handle everything via observers
    //
    // ON INSERT:
    // - add new senders to the ReplicateState
    // - senders that were in ReplicateState but don't match the new Replicate target should be removed.
    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            world.resource_mut::<ReplicableRootEntities>().entities.insert(context.entity);

            // update the authority broker if the entity is spawned on the server
            if let Some(peer_metadata) = world.get_resource::<PeerMetadata>() && let Some(server) = peer_metadata.mapping.get(&PeerId::Server) && let Some(mut broker) = world.get_mut::<AuthorityBroker>(*server) {
                // only set the authority if it didn't have an owner already (in case the authority was replicated
                // by another peer)
                broker.owners.entry(context.entity).or_insert(Some(PeerId::Server));
            }

            let unsafe_world = world.as_unsafe_world_cell();
            // SAFETY: we will use this world to access the ReplicationSender, and the other unsafe_world to access the entity
            let world = unsafe { unsafe_world.world_mut() };
            let mut entity_mut = unsafe { unsafe_world.world_mut() }.entity_mut(context.entity);

            // SAFETY: there is no aliasing because the `entity_mut_state` is used to get these 4 components
            //  and `entity_mut` is used to insert some extra components
            let Ok((mut state, replicate, group)) = (unsafe {
                entity_mut.get_components_mut_unchecked::<(&mut ReplicationState, &Replicate, &ReplicationGroup)>
                ()
            }) else {
                return
            };

            // trackers to check if we need to insert extra components
            // (we cannot insert components on the entity while holding on to `&mut ReplicationState` as the entity would be moved
            // to another archetype
            let mut add_host_client = None;
            let mut add_authority = false;


            let mut add_sender = |senders: &mut EntityIndexMap<PerSenderReplicationState>, sender_entity: Entity, is_host_client: bool| {
                if is_host_client {
                    add_host_client = Some(sender_entity);
                    return;
                }
                // only insert a sender if it was not already present
                // since it could already be present with no_authority (if we received the entity from a remote peer)
                senders.entry(sender_entity)
                    .and_modify(|s| {
                        s.to_remove = false;
                        // authority could be set to None (for example if PredictionTarget is processed first)
                        if s.authority.is_none() {
                            add_authority = true;
                        }
                    })
                    .or_insert_with(|| {
                        trace!("Adding {sender_entity:?} to list of senders for entity {:?} because Replicate is inserted", context.entity);
                        add_authority = true;
                        PerSenderReplicationState::with_authority()
                    });
            };

            // enable split borrows
            match &replicate.mode {
                ReplicationMode::SingleSender => {
                    let Ok((sender_entity, host_client)) = world
                        .query_filtered::<(Entity, Has<HostClient>), Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .single_mut(world)
                    else {
                        error!(entity = ?context.entity, "No ReplicationSender found in the world in mode SingleSender");
                        return;
                    };
                    add_sender(&mut state.per_sender_state, sender_entity, host_client);
                }
                #[cfg(feature = "client")]
                ReplicationMode::SingleClient => {
                    let Ok((sender_entity, host_client)) = world
                        .query_filtered::<
                            (Entity, Has<HostClient>),
                            (With<Client>, Or<(With<ReplicationSender>, With<HostClient>)>)
                        >()
                        .single_mut(world)
                    else {
                        debug!("No Client found in the world");
                        return;
                    };
                    add_sender(&mut state.per_sender_state, sender_entity, host_client);
                }
                #[cfg(feature = "server")]
                ReplicationMode::SingleServer(target) => {
                    use lightyear_connection::client_of::ClientOf;
                    use lightyear_connection::host::HostClient;
                    use lightyear_connection::server::Started;
                    use lightyear_link::server::Server;
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
                    let world = unsafe { unsafe_world.world_mut() };
                    let peer_metadata =
                        world.resource::<PeerMetadata>();
                    let world = unsafe { unsafe_world.world_mut() };
                    target.apply_targets(
                        server.collection().iter().copied(),
                        &peer_metadata.mapping,
                        &mut |client| {
                            let Ok(host_client) = world
                                .query_filtered::<
                                    Has<HostClient>,
                                    (With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>)
                                >()
                                .get_mut(world, client)
                            else {
                                error!("ClientOf {client:?} not found or does not have ReplicationSender");
                                return;
                            };
                            add_sender(&mut state.per_sender_state, client, host_client);
                        },
                    );
                }
                ReplicationMode::Sender(entity) => {
                    let Ok(host_client) = world
                        .query_filtered::<Has<HostClient>, Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .get_mut(world, *entity)
                    else {
                        error!(?entity, "No ReplicationSender found in the world in mode Sender");
                        return;
                    };
                    add_sender(&mut state.per_sender_state, *entity, host_client);
                }
                #[cfg(feature = "server")]
                ReplicationMode::Server(server, target) => {
                    use lightyear_connection::client_of::ClientOf;
                    use lightyear_connection::host::HostClient;
                    use lightyear_connection::server::Started;
                    use lightyear_link::server::Server;
                    use tracing::{debug, error};
                    let unsafe_world = world.as_unsafe_world_cell();
                    // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
                    let entity_ref = unsafe { unsafe_world.world() }.entity(*server);
                    if !entity_ref.contains::<Started>() {
                        error!("Server {:?} was not started", server);
                        return;
                    }
                    let Some(server) = entity_ref.get::<Server>() else {
                        error!(
                            "Provided entity {:?} doesn't have a Server component",
                            server
                        );
                        return;
                    };
                    // SAFETY: we will use this to access the PeerMetadata, which does not alias with the ReplicationSenders
                    let peer_metadata = unsafe { unsafe_world.world() }
                        .resource::<PeerMetadata>();
                    let world = unsafe { unsafe_world.world_mut() };
                    target.apply_targets(
                        server.collection().iter().copied(),
                        &peer_metadata.mapping,
                        &mut |client| {
                            let Ok(host_client) = world
                                .query_filtered::<
                                    Has<HostClient>,
                                    (With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>)
                                >()
                                .get_mut(world, client)
                            else {
                                debug!("ClientOf {client:?} not found or does not have ReplicationSender");
                                return;
                            };
                            add_sender(&mut state.per_sender_state, client, host_client);
                        },
                    );
                }
                ReplicationMode::Target(_) => {
                    todo!(
                        "need a global mapping from remote_peer to corresponding replication_sender"
                    )
                }
                ReplicationMode::Manual(sender_entities) => {
                    for entity in sender_entities.iter() {
                        let Ok(host_client) = world
                            .query_filtered::<
                                Has<HostClient>,
                                Or<(With<ReplicationSender>, With<HostClient>)>>()
                            .get_mut(world, *entity)
                        else {
                            error!(?entity, "No ReplicationSender found in the world in mode Manual");
                            return;
                        };
                        add_sender(&mut state.per_sender_state, *entity, host_client);
                    }
                }
            }

            // Remove senders that were in the previous ReplicationState but don't match the new Replicate target
            state.per_sender_state.retain(|sender_entity, v| {
                if v.to_remove && let Some(mut sender) = world.get_mut::<ReplicationSender>(*sender_entity) {
                    let group_id = group.group_id(Some(context.entity));
                    // TODO: we should also send a despawn for all the replicate-like?
                    sender.set_replicated_despawn(context.entity, group_id);
                }
                !v.to_remove
            });

            if add_authority {
                entity_mut.insert(HasAuthority);
            }
            if let Some(sender_entity) = add_host_client {
                entity_mut.insert((
                     Replicated { receiver: sender_entity },
                     InitialReplicated { receiver: sender_entity },
                     SpawnedOnHostServer,
                ));
            }
        });
    }

    // We don't allow users to manually update Replicate, therefore we can fully rely on observers
    //
    // ON REPLACE:
    // - compare with the previous ReplicationState to identify which senders the entity should be despawned on
    fn on_replace(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            world
                .resource_mut::<ReplicableRootEntities>()
                .entities
                .swap_remove(&context.entity);
            if let Ok(mut entity_mut) = world.get_entity_mut(context.entity) {
                entity_mut
                    .get_mut::<ReplicationState>()
                    .unwrap()
                    .per_sender_state
                    .values_mut()
                    .for_each(|state| {
                        if state.spawned {
                            // we need to remove this sender if it's not added in the next replicate
                            // we don't remove them here because we want to keep the metadata if the sender is kept
                            state.to_remove = true;
                        }
                    });
            }
        });
    }
}

#[derive(QueryData)]
#[query_data(mutable)]
pub(super) struct HandleConnectionQueryData {
    entity: Entity,
    replicate: &'static Replicate,
    #[cfg(feature = "prediction")]
    prediction: Option<&'static PredictionTarget>,
    #[cfg(feature = "interpolation")]
    interpolation: Option<&'static InterpolationTarget>,
    state: &'static mut ReplicationState,
}

impl Replicate {
    /// When a new client connects, check if we need to replicate existing entities to it
    pub(super) fn handle_connection(
        trigger: On<Add, (Connected, ReplicationSender)>,
        mut sender_query: Query<
            (Entity, &RemoteId, Has<Client>, Option<&LinkOf>),
            // no need to replicate to the HostClient
            (With<Connected>, Without<HostClient>),
        >,
        mut replicate_query: Query<HandleConnectionQueryData>,
        mut commands: Commands,
    ) {
        fn update_replicate(
            state: &mut Mut<ReplicationState>,
            entity: Entity,
            sender_entity: Entity,
            commands: &mut Commands,
        ) {
            state
                .per_sender_state
                .entry(sender_entity)
                .or_insert_with(|| {
                    commands.entity(entity).insert_if_new(HasAuthority);
                    PerSenderReplicationState::with_authority()
                });
        }
        #[cfg(feature = "prediction")]
        fn update_prediction(
            state: &mut Mut<ReplicationState>,
            entity: Entity,
            sender_entity: Entity,
            _: &mut Commands,
        ) {
            state
                .per_sender_state
                .entry(sender_entity)
                .and_modify(|s| s.predicted = true);
        }
        #[cfg(feature = "interpolation")]
        fn update_interpolation(
            state: &mut Mut<ReplicationState>,
            entity: Entity,
            sender_entity: Entity,
            _: &mut Commands,
        ) {
            state
                .per_sender_state
                .entry(sender_entity)
                .and_modify(|s| s.interpolated = true);
        }

        if let Ok((sender_entity, remote_peer_id, _client, client_of)) =
            sender_query.get_mut(trigger.entity)
        {
            // TODO: maybe do this in parallel?
            replicate_query.iter_mut().for_each(
                |HandleConnectionQueryDataItem {
                     entity,
                     replicate,
                     #[cfg(feature = "prediction")]
                     prediction,
                     #[cfg(feature = "interpolation")]
                     interpolation,
                     mut state,
                 }| {
                    let state = &mut state;
                    let mut update_state = |mode: &ReplicationMode,
                                            f: fn(
                        &mut Mut<ReplicationState>,
                        Entity,
                        Entity,
                        &mut Commands,
                    )| {
                        match mode {
                            ReplicationMode::SingleSender => {
                                todo!()
                            }
                            #[cfg(feature = "client")]
                            ReplicationMode::SingleClient => {
                                // this can only happen if we are in host-server mode. In which case we don't want to replicate on other clients
                            }
                            #[cfg(feature = "server")]
                            ReplicationMode::SingleServer(target) => {
                                if client_of.is_some() && target.targets(remote_peer_id) {
                                    f(state, entity, sender_entity, &mut commands);
                                }
                            }
                            ReplicationMode::Sender(_) => {
                                todo!()
                            }
                            #[cfg(feature = "server")]
                            ReplicationMode::Server(e, target) => {
                                if client_of.is_some_and(|c| c.server == *e)
                                    && target.targets(remote_peer_id)
                                {
                                    f(state, entity, sender_entity, &mut commands);
                                }
                            }
                            ReplicationMode::Target(target) => {
                                if target.targets(remote_peer_id) {
                                    f(state, entity, sender_entity, &mut commands);
                                }
                            }
                            ReplicationMode::Manual(entities) => {
                                if entities.contains(&sender_entity) {
                                    f(state, entity, sender_entity, &mut commands);
                                };
                            }
                        }
                    };

                    update_state(&replicate.mode, update_replicate);
                    #[cfg(feature = "prediction")]
                    if let Some(prediction) = prediction {
                        update_state(&prediction.mode, update_prediction);
                    }
                    #[cfg(feature = "interpolation")]
                    if let Some(interpolation) = interpolation {
                        update_state(&interpolation.mode, update_interpolation);
                    }
                },
            );
        }
    }
}

// TODO: add unit tests for each of these
// Some scenarios that we want to handle:
// - 1) New entity with Replicate spawns: we want to replicate it to the senders.
// - 2) New client connects: we want to replicate existing entities to it. Also we want to update entities' Replicate
//   to include that new sender. This update the Replicate component, but we don't to re-send the entity to clients
//   it was already replicated to.
// - 3) Entity with Replicate is removed: we want to despawn it on all senders that had it spawned
// - 4) Client disconnects: we want to remove the sender from the list
//
// - 5) New Replicate is added that removes some senders: we use `to_remove` to check which senders we can remove
// - 6) New Replicate is added that adds some senders: we use `spawned` to check if we need to spawn the entity on the new senders
//
// - 7) ReplicateLike is added on an entity
//
// 1) new entity with Replicate spawns:
//    - replicate.changed() is true -> we check all senders and we check if sender has already spawned the entity.
// 2) new client connects:
//    - we add the new client to Replicate
//    - we add the entity to sender.replicated_entities with "not spawned"
// 3) entity with Replicate is removed:
//    - replicate.removed() is true -> we check all senders and we check if sender has spawned the entity. If yes, we despawn
// 4) client disconnects:
//    - we remove the sender from Replicate
// 5) new replicate is added that removes some senders:
// 6) new replicate is added that adds some senders:

#[cfg(test)]
mod tests {
    use crate::prelude::ReplicationGroupId;
    use bevy_ecs::entity::Entity;
    use lightyear_serde::ToBytes;
    use lightyear_serde::reader::Reader;
    use lightyear_serde::writer::Writer;

    #[test]
    fn test_replication_group_id_serde() {
        let group_0 = ReplicationGroupId(0);
        let mut writer = Writer::with_capacity(100);
        group_0.to_bytes(&mut writer).unwrap();
        assert_eq!(writer.len(), 1);
        let mut reader = Reader::from(writer.split());
        let serde_group_0 = ReplicationGroupId::from_bytes(&mut reader).unwrap();
        assert_eq!(group_0, serde_group_0);

        let entity = Entity::from_raw_u32(10).unwrap();
        let group_1 = ReplicationGroupId(entity.to_bits());
        let mut writer = Writer::with_capacity(100);
        group_1.to_bytes(&mut writer).unwrap();
        assert_eq!(writer.len(), 1);
        let mut reader = Reader::from(writer.split());
        let serde_group_1 = ReplicationGroupId::from_bytes(&mut reader).unwrap();
        assert_eq!(group_1, serde_group_1);
    }
}
