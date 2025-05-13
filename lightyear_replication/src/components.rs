//! Components used for replication

use crate::buffer::{Replicate, ReplicationMode};
use crate::prelude::ReplicationSender;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::ecs::component::HookContext;
use bevy::ecs::entity::EntityIndexSet;
use bevy::ecs::reflect::ReflectComponent;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::time::{Timer, TimerMode};
use lightyear_connection::client::{Connected, PeerMetadata};
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::id::PeerId;
use lightyear_core::tick::Tick;
use lightyear_link::prelude::LinkOf;
#[cfg(feature = "server")]
use lightyear_link::server::Server;
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_utils::collections::EntityHashMap;
use serde::{Deserialize, Serialize};
// TODO: how to define which subset of components a sender iterates through?
//  if a sender is only interested in a few components it might be expensive
//  maybe we can have a 'direction' in ComponentReplicationConfig and Client/ClientOf peers can precompute
//  a list of components based on this.

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
}

#[derive(Component, Reflect)]
pub struct ComponentReplicationOverrides<C> {
    /// Overrides that will be applied to all senders
    pub(crate) all_senders: Option<ComponentReplicationOverride>,
    /// Overrides that will be applied for a specific sender. Takes priority over `all_senders`
    pub(crate) per_sender: EntityHashMap<ComponentReplicationOverride>,
    _marker: core::marker::PhantomData<C>,
}

impl<C> Default for ComponentReplicationOverrides<C> {
    fn default() -> Self {
        Self {
            all_senders: None,
            per_sender: Default::default(),
            _marker: core::marker::PhantomData,
        }
    }
}

impl<C> ComponentReplicationOverrides<C> {
    /// Get component overrides for a specific sender
    pub(crate) fn get_overrides(&self, sender: Entity) -> Option<&ComponentReplicationOverride> {
        if let Some(overrides) = self.per_sender.get(&sender) {
            return Some(overrides);
        }
        self.all_senders.as_ref()
    }

    /// Add an override for all senders
    pub fn global_override(&mut self, overrides: ComponentReplicationOverride) {
        self.all_senders = Some(overrides);
    }

    /// Add an override for a specific sender. Takes priority over any global override
    pub fn override_for_sender(&mut self, overrides: ComponentReplicationOverride, sender: Entity) {
        self.per_sender.insert(sender, overrides);
    }
}

/// Marker component that indicates that the entity was initially spawned via replication
/// (it was being replicated from a remote world)
///
/// The component is added once and is then never modified anymore
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct InitialReplicated {
    /// The peer that originally spawned the entity
    pub from: PeerId,
}

/// Marker component that indicates that the entity is being replicated
/// from a remote world.
///
/// The component only exists while the peer does not have authority over
/// the entity.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct Replicated {
    /// Entity that holds the ReplicationReceiver for this entity
    pub receiver: Entity,
    /// The remote peer that is actively replicating the entity
    pub from: PeerId,
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

// TODO: we need a ReplicateConfig similar to ComponentReplicationConfig
//  for entity-specific config, such as replicate-hierarchy

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
    pub(crate) send_frequency: Option<Timer>,
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
    pub(crate) should_send: bool,
}

impl Default for ReplicationGroup {
    fn default() -> Self {
        Self {
            id_builder: ReplicationGroupIdBuilder::FromEntity,
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

    pub(crate) fn group_id(&self, entity: Option<Entity>) -> ReplicationGroupId {
        match self.id_builder {
            ReplicationGroupIdBuilder::FromEntity => {
                ReplicationGroupId(entity.expect("need to provide an entity").to_bits())
            }
            ReplicationGroupIdBuilder::Group(id) => ReplicationGroupId(id),
        }
    }

    pub(crate) fn priority(&self) -> f32 {
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
        8
        // TODO: if it's a valid entity (generation > 0 and high-bit is 0)
        //  optimize by serializing as an entity!
        // Entity::try_from_bits(self.0).map_or_else(|_| 8, |entity| entity.bytes_len())
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        // Entity::try_from_bits(self.0).map_or_else(|_| {)
        //     buffer.write_u64(self.0)?;
        //     Ok(())
        // }, |entity| {
        //     entity.to_bytes(buffer)
        // })?;
        buffer.write_u64(self.0)?;
        // Entity::to_bytes(&Entity::from_bits(self.0), buffer)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(Self(buffer.read_u64()?))
        // let entity = Entity::from_bytes(buffer)?;
        // Ok(Self(entity.to_bits()))
    }
}

/// Marks an entity that directly applies the replication updates from the remote
///
/// In general, when an entity is replicated from the server to the client, multiple entities can be created on the client:
/// - an entity that simply contains the replicated components. It will have the marker component [`Confirmed`]
/// - an entity that is in the future compared to the confirmed entity, and does prediction with rollback. It will have the marker component [`Predicted`](crate::client::prediction::Predicted)
/// - an entity that is in the past compared to the confirmed entity and interpolates between multiple server updates. It will have the marker component [`Interpolated`](crate::client::interpolation::Interpolated)
#[derive(Component, Reflect, Default, Debug)]
#[reflect(Component)]
pub struct Confirmed {
    /// The corresponding Predicted entity
    pub predicted: Option<Entity>,
    /// The corresponding Interpolated entity
    pub interpolated: Option<Entity>,
    /// The tick that the confirmed entity is at.
    /// (this is latest server tick for which we applied updates to the entity)
    pub tick: Tick,
}

// TODO: enable this only if predicted feature
/// Indicates that an entity was pre-predicted
// NOTE: we do not map entities for this component, we want to receive the entities as is
//  because we already do the mapping at other steps
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct PrePredicted {
    pub(crate) confirmed_entity: Option<Entity>,
}

#[cfg(feature = "prediction")]
/// Marker component that tells the client to spawn a Predicted entity
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct ShouldBePredicted;

#[cfg(feature = "prediction")]
pub type PredictionTarget = ReplicationTarget<ShouldBePredicted>;

#[cfg(feature = "interpolation")]
/// Marker component that tells the client to spawn an Interpolated entity
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct ShouldBeInterpolated;

#[cfg(feature = "interpolation")]
pub type InterpolationTarget = ReplicationTarget<ShouldBeInterpolated>;

/// Insert this component to specify which remote peers will start predicting the entity
/// upon receiving the entity.
#[derive(Component, Clone, Default, Debug, PartialEq, Reflect)]
#[require(Replicate)]
#[component(on_insert = ReplicationTarget::<T>::on_insert)]
#[component(on_replace = ReplicationTarget::<T>::on_replace)]
pub struct ReplicationTarget<T: Sync + Send + 'static> {
    mode: ReplicationMode,
    #[reflect(ignore)]
    pub(crate) senders: EntityIndexSet,
    #[reflect(ignore)]
    marker: core::marker::PhantomData<T>,
}

impl<T: Sync + Send + 'static> ReplicationTarget<T> {
    pub fn new(mode: ReplicationMode) -> Self {
        Self {
            mode,
            senders: EntityIndexSet::default(),
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

    /// List of [`ReplicationSender`] entities that this entity targets
    pub fn senders(&self) -> impl Iterator<Item = Entity> {
        self.senders.iter().copied()
    }

    pub fn on_insert(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let unsafe_world = world.as_unsafe_world_cell();
            // SAFETY: we will use this world to access the ReplicationSender
            let world = unsafe { unsafe_world.world_mut() };
            // SAFETY: we will use this world only to access the Replicated entity, so there is no aliasing issue
            let mut replicate_entity_mut =
                unsafe { unsafe_world.world_mut().entity_mut(context.entity) };

            let mut replicate = replicate_entity_mut
                .get_mut::<ReplicationTarget<T>>()
                .unwrap();
            // enable split borrows
            let replicate = &mut *replicate;
            match &mut replicate.mode {
                ReplicationMode::SingleSender => {
                    let Ok(sender_entity) = world.query::<Entity>().single_mut(world) else {
                        error!("No ReplicationSender found in the world");
                        return;
                    };
                    replicate.senders.insert(sender_entity);
                }
                #[cfg(feature = "client")]
                ReplicationMode::SingleClient => {
                    use lightyear_connection::client::Client;
                    let Ok(sender_entity) = world
                        .query_filtered::<Entity, With<Client>>()
                        .single_mut(world)
                    else {
                        error!("No Client found in the world");
                        return;
                    };
                    debug!(
                        "Adding replicated entity {} to sender {}",
                        context.entity, sender_entity
                    );
                    replicate.senders.insert(sender_entity);
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
                        error!("No Server found in the world");
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
                                core::any::type_name::<T>(),
                                context.entity,
                                client
                            );
                            let Ok(()) = world
                                .query_filtered::<(), (With<ClientOf>, With<ReplicationSender>)>()
                                .get_mut(world, client)
                            else {
                                error!("ClientOf {client:?} not found");
                                return;
                            };
                            replicate.senders.insert(client);
                        },
                    );
                }
                ReplicationMode::Sender(entity) => {
                    let Ok(()) = world
                        .query_filtered::<(), With<ReplicationSender>>()
                        .get_mut(world, *entity)
                    else {
                        error!("No ReplicationSender found in the world");
                        return;
                    };
                    replicate.senders.insert(*entity);
                }
                #[cfg(feature = "server")]
                ReplicationMode::Server(server, target) => {
                    let unsafe_world = world.as_unsafe_world_cell();
                    // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
                    let entity_ref = unsafe { unsafe_world.world() }.entity(*server);
                    if !entity_ref.contains::<Server>() {
                        error!("No Server found in the world");
                        return;
                    }
                    let Some(server) = entity_ref.get::<Server>() else {
                        error!("No Server found in the world");
                        return;
                    };
                    // SAFETY: we will use this to access the PeerMetadata, which does not alias with the ReplicationSenders
                    let peer_metadata = unsafe { unsafe_world.world() }.resource::<PeerMetadata>();
                    let world = unsafe { unsafe_world.world_mut() };
                    target.apply_targets(
                        server.collection().iter().copied(),
                        &peer_metadata.mapping,
                        &mut |client| {
                            let Ok(()) = world
                                .query_filtered::<(), (With<ClientOf>, With<ReplicationSender>)>()
                                .get_mut(world, client)
                            else {
                                error!("No Client found in the world");
                                return;
                            };
                            replicate.senders.insert(client);
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
                        let Ok(()) = world
                            .query_filtered::<(), With<ReplicationSender>>()
                            .get_mut(world, *sender_entity)
                        else {
                            error!("No ReplicationSender found in the world");
                            return;
                        };
                        replicate.senders.insert(*sender_entity);
                    }
                }
            }
        });
    }

    pub fn on_replace(mut world: DeferredWorld, context: HookContext) {
        // TODO: maybe we can just use the CachedReplicate?
        // i.e. if you remove 2 clients from Replicate, than in PreBuffer, we will do the diff
        // and remove those clients from sender.replicated_entities and send the despawn

        let mut replicate = world
            .get_mut::<ReplicationTarget<T>>(context.entity)
            .unwrap();
        replicate.senders = EntityIndexSet::default();
    }

    #[cfg(any(feature = "client", feature = "server"))]
    /// When a new client connects, check if we need to replicate existing entities to it
    pub(crate) fn handle_connection(
        trigger: Trigger<OnAdd, (Connected, ReplicationSender)>,
        mut sender_query: Query<(Entity, &mut ReplicationSender, &Connected, Option<&LinkOf>)>,
        mut replicate_query: Query<(Entity, &mut ReplicationTarget<T>)>,
    ) {
        if let Ok((sender_entity, mut sender, connected, link_of)) =
            sender_query.get_mut(trigger.target())
        {
            // TODO: maybe do this in parallel?
            replicate_query.iter_mut().for_each(|(entity, mut replicate)| {
                match &replicate.mode {
                    ReplicationMode::SingleSender => {}
                    #[cfg(feature = "client")]
                    ReplicationMode::SingleClient => {}
                    #[cfg(feature = "server")]
                    ReplicationMode::SingleServer(target) => {
                        if link_of.is_some() && target.targets(&connected.remote_peer_id) {
                            info!("Replicating existing entity {entity:?} to newly connected sender {sender_entity:?}");
                            sender.add_replicated_entity(entity, true);
                            replicate.senders.insert(sender_entity);
                        }
                    }
                    ReplicationMode::Sender(_) => {}
                    #[cfg(feature = "server")]
                    ReplicationMode::Server(e, target) => {
                        if target.targets(&connected.remote_peer_id) && link_of.is_some_and(|c| c.server == *e) {
                            sender.add_replicated_entity(entity, true);
                            replicate.senders.insert(sender_entity);
                        }
                    }
                    ReplicationMode::Target(target) => {
                        if target.targets(&connected.remote_peer_id) {
                            sender.add_replicated_entity(entity, true);
                            replicate.senders.insert(sender_entity);
                        }
                    }
                    ReplicationMode::Manual(_) => {}
                }
            })
        }
    }
}
