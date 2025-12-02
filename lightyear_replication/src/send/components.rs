use crate::authority::{AuthorityBroker, HasAuthority};
use crate::components::ComponentReplicationOverrides;
use crate::send::sender::{ReplicationSender, ReplicationStatus};
use alloc::vec::Vec;
use bevy_ecs::component::Component;
use bevy_ecs::entity::index_set::EntityIndexSet;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::reflect::ReflectComponent;
use bevy_ecs::world::DeferredWorld;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_reflect::Reflect;
use bevy_time::{Timer, TimerMode};
use bevy_utils::prelude::DebugName;
use lightyear_connection::client::{Client, Connected, PeerMetadata};
#[cfg(feature = "server")]
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::host::HostClient;
use lightyear_connection::network_target::NetworkTarget;

use lightyear_core::id::{PeerId, RemoteId};
use lightyear_link::server::LinkOf;
#[cfg(feature = "server")]
use lightyear_link::server::Server;
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

pub type InterpolationTarget = ReplicationTarget<lightyear_core::interpolation::Interpolated>;

/// Insert this component to specify which remote peers will start predicting the entity
/// upon receiving the entity.
// NOTE: we don't require Replicate here because we might be using this with ReplicateLike entities
//  in order to override the prediction/interpolation targets.
#[derive(Component, Clone, Default, Debug, PartialEq, Reflect)]
#[component(on_insert = ReplicationTarget::<T>::on_insert)]
#[component(on_replace = ReplicationTarget::<T>::on_replace)]
pub struct ReplicationTarget<T: Sync + Send + 'static> {
    mode: ReplicationMode,
    #[reflect(ignore)]
    pub senders: EntityIndexSet,
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

    pub(crate) fn on_insert(mut world: DeferredWorld, context: HookContext) {
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
                    let Ok(sender_entity) = world.query_filtered::<Entity, Or<(With<ReplicationSender>, With<HostClient>)>>().single_mut(world) else {
                        error!(mode = ?replicate.mode, "No ReplicationSender found in the world");
                        return;
                    };
                    replicate.senders.insert(sender_entity);
                }
                #[cfg(feature = "client")]
                ReplicationMode::SingleClient => {
                    use lightyear_connection::client::Client;
                    use lightyear_connection::host::HostClient;
                    use tracing::{debug};
                    let Ok(sender_entity) = world
                        .query_filtered::<Entity, (With<Client>, Or<(With<ReplicationSender>, With<HostClient>)>)>()
                        .single_mut(world)
                    else {
                        debug!("No Client found in the world");
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
                            let Ok(()) = world
                                .query_filtered::<(), (
                                    With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>
                                )>()
                                .get_mut(world, client)
                            else {
                                error!("ClientOf {client:?} not found or does not have ReplicationSender");
                                return;
                            };
                            replicate.senders.insert(client);
                        },
                    );
                }
                ReplicationMode::Sender(entity) => {
                    let Ok(()) = world
                        .query_filtered::<(), Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .get_mut(world, *entity)
                    else {
                        error!(mode = ?replicate.mode, "No ReplicationSender found in the world");
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
                            let Ok(()) = world
                                .query_filtered::<(), (With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>)>()
                                .get_mut(world, client)
                            else {
                                debug!("No Client found in the world");
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
                            .query_filtered::<(), Or<(With<ReplicationSender>, With<HostClient>)>>()
                            .get_mut(world, *sender_entity)
                        else {
                            error!(mode = ?replicate.mode, "No ReplicationSender found in the world for target: {:?}", DebugName::type_name::<T>());
                            return;
                        };
                        replicate.senders.insert(*sender_entity);
                    }
                }
            }
        });
    }

    pub(crate) fn on_replace(mut world: DeferredWorld, context: HookContext) {
        let mut replicate = world
            .get_mut::<ReplicationTarget<T>>(context.entity)
            .unwrap();
        replicate.senders = EntityIndexSet::default();
    }

    /// When a new client connects, check if we need to replicate existing entities to it
    pub(crate) fn handle_connection(
        trigger: On<Add, (Connected, ReplicationSender)>,
        mut sender_query: Query<
            (Entity, &mut ReplicationSender, &RemoteId, Option<&LinkOf>),
            With<Connected>,
        >,
        mut replicate_query: Query<(Entity, &mut ReplicationTarget<T>)>,
        mut commands: Commands,
    ) {
        if let Ok((sender_entity, mut sender, remote_peer_id, link_of)) =
            sender_query.get_mut(trigger.entity)
        {
            // TODO: maybe do this in parallel?
            replicate_query.iter_mut().for_each(|(entity, mut replicate)| {
                match &replicate.mode {
                    ReplicationMode::SingleSender => {}
                    #[cfg(feature = "client")]
                    ReplicationMode::SingleClient => {}
                    #[cfg(feature = "server")]
                    ReplicationMode::SingleServer(target) => {
                        if link_of.is_some() && target.targets(remote_peer_id) {
                            debug!("Replicating existing entity {entity:?} to newly connected sender {sender_entity:?}");
                            if !sender.replicated_entities.contains_key(&entity) {
                                sender.replicated_entities.insert(entity, ReplicationStatus {
                                    authority: true,
                                    spawned: false,
                                });
                                commands.entity(entity).insert_if_new(HasAuthority);
                            }
                            replicate.senders.insert(sender_entity);
                        }
                    }
                    ReplicationMode::Sender(_) => {}
                    #[cfg(feature = "server")]
                    ReplicationMode::Server(e, target) => {
                        if target.targets(remote_peer_id) && link_of.is_some_and(|c| c.server == *e) {
                            if !sender.replicated_entities.contains_key(&entity) {
                                sender.replicated_entities.insert(entity, ReplicationStatus {
                                    authority: true,
                                    spawned: false,
                                });
                                commands.entity(entity).insert_if_new(HasAuthority);
                            }
                            replicate.senders.insert(sender_entity);
                        }
                    }
                    ReplicationMode::Target(target) => {
                        if target.targets(remote_peer_id) {
                            if !sender.replicated_entities.contains_key(&entity) {
                                sender.replicated_entities.insert(entity, ReplicationStatus {
                                    authority: true,
                                    spawned: false,
                                });
                                commands.entity(entity).insert_if_new(HasAuthority);
                            }
                            replicate.senders.insert(sender_entity);
                        }
                    }
                    ReplicationMode::Manual(_) => {}
                }
            })
        }
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

/// Insert this component to start replicating your entity.
///
/// - If sender is an Entity that has a ReplicationSender, we will replicate on that entity
/// - If the entity is None, we will try to find a unique ReplicationSender in the app
#[derive(Component, Clone, Default, Debug, PartialEq, Reflect)]
#[require(Replicating)]
#[require(ReplicationGroup)]
#[component(on_insert = Replicate::on_insert)]
#[component(on_replace = Replicate::on_replace)]
#[reflect(Component)]
pub struct Replicate {
    /// Defines which [`ReplicationSenders`](ReplicationSender) this entity will be replicated to
    mode: ReplicationMode,
    /// The list of [`ReplicationSender`] entities that this entity is being replicated on
    #[reflect(ignore)]
    pub(crate) senders: EntityIndexSet,
}

impl Replicate {
    pub fn new(mode: ReplicationMode) -> Self {
        Self {
            mode,
            senders: EntityIndexSet::default(),
        }
    }

    #[cfg(feature = "client")]
    pub fn to_server() -> Self {
        Self {
            mode: ReplicationMode::SingleClient,
            senders: EntityIndexSet::default(),
        }
    }

    #[cfg(feature = "server")]
    pub fn to_clients(target: NetworkTarget) -> Self {
        Self {
            mode: ReplicationMode::SingleServer(target),
            senders: EntityIndexSet::default(),
        }
    }

    pub fn manual(senders: Vec<Entity>) -> Self {
        Self {
            mode: ReplicationMode::Manual(senders),
            senders: EntityIndexSet::default(),
        }
    }

    /// List of [`ReplicationSender`] entities that this entity is being replicated on
    pub fn senders(&self) -> impl Iterator<Item = Entity> {
        self.senders.iter().copied()
    }

    // We NEVER manually update replicate, so we can handle everything via observers
    //
    // ON REPLACE: (on drop)
    // - store the previous state of Replicate in CachedReplicate
    //
    // ON INSERT:
    // - check each newly added sender with CachedReplicate to see if we removed/added some senders
    //   - removed senders: get added to `sender.removed_entities` if we had spawned the entity on that sender
    //   - added senders: get added to `sender.replicated_entities` if it was not in the previous CachedReplicate
    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let unsafe_world = world.as_unsafe_world_cell();

            // SAFETY: we will use this world to access the ReplicationSender
            let world = unsafe { unsafe_world.world_mut() };

            let unsafe_entity_cell = unsafe_world.get_entity(context.entity).unwrap();
            // SAFETY: there is no aliasing because we only access Replicate and CachedReplicate
            let mut replicate = unsafe { unsafe_entity_cell.get_mut::<Replicate>().unwrap() };
            let cached_replicate = unsafe { unsafe_entity_cell.get::<CachedReplicate>() };

            // update the authority broker if the entity is spawned on the server
            if let Some(peer_metadata) = world.get_resource::<PeerMetadata>() && let Some(server) = peer_metadata.mapping.get(&PeerId::Server) && let Some(mut broker) = world.get_mut::<AuthorityBroker>(*server) {
                // only set the authority if it didn't have an owner already (in case the authority was replicated
                // by another peer)
                broker.owners.entry(context.entity).or_insert(Some(PeerId::Server));
            }

            let add_sender = |senders: &mut EntityIndexSet, cached_replicate: Option<&CachedReplicate>, sender_entity: Entity, sender: Option<Mut<ReplicationSender>>, unsafe_world: UnsafeWorldCell| {
                let Some(mut sender) = sender else {
                    error!(?sender_entity, "No ReplicationSender found in the world in mode Manual");
                    return;
                };
                // Add senders that were not in the previous CachedReplicate
                senders.insert(sender_entity);
                if cached_replicate.is_none_or(|c| !c.senders.contains(&sender_entity)) {
                    trace!("Adding new sender {} for replicated entity {}", sender_entity, context.entity);
                    // if the entity was already in sender.replicated_entities (because we received it from another peer)
                    // don't update the authority
                    if !sender.replicated_entities.contains_key(&context.entity) {
                        sender.replicated_entities.insert(context.entity, ReplicationStatus {
                            authority: true,
                            spawned: false,
                        });
                        trace!("Adding HasAuthority to entity {:?} because Replicate is inserted", context.entity);
                        // SAFETY: we are not doing aliasing
                        unsafe { unsafe_world.world_mut() }.entity_mut(context.entity).insert_if_new(HasAuthority);
                    };

                }
            };

            // enable split borrows
            let replicate = &mut *replicate;
            match &replicate.mode {
                ReplicationMode::SingleSender => {
                    let Ok((sender_entity, sender, host_client)) = world
                        .query_filtered::<(Entity, Option<&mut ReplicationSender>, Has<HostClient>), Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .single_mut(world)
                    else {
                        error!(entity = ?context.entity, "No ReplicationSender found in the world in mode SingleSender");
                        return;
                    };
                    if host_client {
                        return;
                    }
                    add_sender(&mut replicate.senders, cached_replicate, sender_entity, sender, unsafe_world);
                }
                #[cfg(feature = "client")]
                ReplicationMode::SingleClient => {
                    let Ok((sender_entity, sender, host_client)) = world
                        .query_filtered::<
                            (Entity, Option<&mut ReplicationSender>, Has<HostClient>),
                            (With<Client>, Or<(With<ReplicationSender>, With<HostClient>)>)
                        >()
                        .single_mut(world)
                    else {
                        debug!("No Client found in the world");
                        return;
                    };
                    debug!(
                        "Adding replicated entity {} to sender {}",
                        context.entity, sender_entity
                    );
                    // TODO: maybe we should update `sender` even if it's a HostClient since it might be needed
                    //  to insert the fake replication components for host-server
                    if host_client {
                        return;
                    }
                    add_sender(&mut replicate.senders, cached_replicate, sender_entity, sender, unsafe_world);
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
                            let Ok((sender, host_client)) = world
                                .query_filtered::<
                                    (Option<&mut ReplicationSender>, Has<HostClient>),
                                    (With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>)
                                >()
                                .get_mut(world, client)
                            else {
                                error!("ClientOf {client:?} not found or does not have ReplicationSender");
                                return;
                            };
                            if host_client {
                                return;
                            }
                            add_sender(&mut replicate.senders, cached_replicate, client, sender, unsafe_world);
                        },
                    );
                }
                ReplicationMode::Sender(entity) => {
                    let Ok((sender, host_client)) = world
                        .query_filtered::<(Option<&mut ReplicationSender>, Has<HostClient>), Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .get_mut(world, *entity)
                    else {
                        error!(?entity, "No ReplicationSender found in the world in mode Sender");
                        return;
                    };
                    if host_client {
                        return;
                    }
                    add_sender(&mut replicate.senders, cached_replicate, *entity, sender, unsafe_world);
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
                        error!("Server {:?} was not started", *server);
                        return;
                    }
                    let Some(server) = entity_ref.get::<Server>() else {
                        error!(
                            "Provided entity {:?} doesn't have a Server component",
                            *server
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
                            let Ok((sender, host_client)) = world
                                .query_filtered::<
                                    (Option<&mut ReplicationSender>, Has<HostClient>),
                                    (With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>)
                                >()
                                .get_mut(world, client)
                            else {
                                debug!("ClientOf {client:?} not found or does not have ReplicationSender");
                                return;
                            };
                            if host_client {
                                return;
                            }
                            add_sender(&mut replicate.senders, cached_replicate, client, sender, unsafe_world);
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
                         let Ok((sender, host_client)) = world
                        .query_filtered::<
                            (Option<&mut ReplicationSender>, Has<HostClient>),
                            Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .get_mut(world, *entity)
                        else {
                            error!(?entity, "No ReplicationSender found in the world in mode Manual");
                            return;
                        };
                        if host_client {
                            return;
                        }
                        add_sender(&mut replicate.senders, cached_replicate, *entity, sender, unsafe_world);
                    }
                }
            }

            // Remove senders that were in the previous CachedReplicate but are not in the new Replicate
            if let Some(cached_replicate) = cached_replicate {
                cached_replicate.senders.iter().for_each(|sender_entity| {
                    if !replicate.senders.contains(sender_entity) && let Some(mut sender) = world.get_mut::<ReplicationSender>(*sender_entity) {
                        let group_id = unsafe { unsafe_entity_cell.get::<ReplicationGroup>().unwrap() }.group_id(Some(context.entity));
                        trace!("Removing sender {} for replicated entity {}", sender_entity, context.entity);
                        sender.set_replicated_despawn(context.entity, group_id);
                    }
                });
            }
        });
    }

    // We don't allow users to manually update replicate, so CachedReplicate can be updated in observers.
    //
    // ON REPLACE:
    // - store the previous state of Replicate in CachedReplicate
    fn on_replace(mut world: DeferredWorld, context: HookContext) {
        let replicate = world.get::<Replicate>(context.entity).unwrap().clone();
        world.commands().queue(move |world: &mut World| {
            if let Ok(mut entity_mut) = world.get_entity_mut(context.entity) {
                entity_mut.insert(CachedReplicate {
                    senders: replicate.senders,
                });
            }
        });
    }

    /// When a new client connects, check if we need to replicate existing entities to it
    pub(crate) fn handle_connection(
        trigger: On<Add, (Connected, ReplicationSender)>,
        mut sender_query: Query<
            (
                Entity,
                &mut ReplicationSender,
                &RemoteId,
                Has<Client>,
                Option<&LinkOf>,
            ),
            With<Connected>,
        >,
        mut replicate_query: Query<(Entity, &mut Replicate, Option<&mut CachedReplicate>)>,
        mut commands: Commands,
    ) {
        if let Ok((sender_entity, mut sender, remote_peer_id, _client, client_of)) =
            sender_query.get_mut(trigger.entity)
        {
            // TODO: maybe do this in parallel?
            replicate_query.iter_mut().for_each(|(entity, mut replicate, mut cached_replicate)| {
                match &replicate.mode {
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
                            debug!("Replicating existing entity {entity:?} to newly connected sender {sender_entity:?}");
                            if !sender.replicated_entities.contains_key(&entity) {
                                sender.replicated_entities.insert(entity, ReplicationStatus {
                                    authority: true,
                                    spawned: false,
                                });
                                commands.entity(entity).insert_if_new(HasAuthority);
                            }
                            replicate.senders.insert(sender_entity);
                            // we also update the Cache, so that it's correct if a new Replicate is inserted
                            if let Some(cached_replicate) = cached_replicate.as_mut() {
                                cached_replicate.senders.insert(sender_entity);
                            }
                        }
                    }
                    ReplicationMode::Sender(_) => {
                        todo!()
                    }
                    #[cfg(feature = "server")]
                    ReplicationMode::Server(e, target) => {
                        if client_of.is_some_and(|c| c.server == *e) && target.targets(remote_peer_id) {
                            if !sender.replicated_entities.contains_key(&entity) {
                                sender.replicated_entities.insert(entity, ReplicationStatus {
                                    authority: true,
                                    spawned: false,
                                });
                                commands.entity(entity).insert_if_new(HasAuthority);
                            }
                            replicate.senders.insert(sender_entity);
                            if let Some(cached_replicate) = cached_replicate.as_mut() {
                                cached_replicate.senders.insert(sender_entity);
                            }
                        }
                    }
                    ReplicationMode::Target(target) => {
                        if target.targets(remote_peer_id) {
                            if !sender.replicated_entities.contains_key(&entity) {
                                sender.replicated_entities.insert(entity, ReplicationStatus {
                                    authority: true,
                                    spawned: false,
                                });
                                commands.entity(entity).insert_if_new(HasAuthority);
                            }

                            replicate.senders.insert(sender_entity);
                            if let Some(cached_replicate) = cached_replicate.as_mut() {
                                cached_replicate.senders.insert(sender_entity);
                            }
                        }
                    }
                    ReplicationMode::Manual(entities) => {
                        if entities.contains(&sender_entity) {
                            if !sender.replicated_entities.contains_key(&entity) {
                                sender.replicated_entities.insert(entity, ReplicationStatus {
                                    authority: true,
                                    spawned: false,
                                });
                                commands.entity(entity).insert_if_new(HasAuthority);
                            }
                            replicate.senders.insert(sender_entity);
                            if let Some(cached_replicate) = cached_replicate.as_mut() {
                                cached_replicate.senders.insert(sender_entity);
                            }
                        };
                    }
                }
            })
        }
    }
}

/// Internal component to cache which senders an entity was previously replicated to

#[derive(Component, Debug)]
pub(crate) struct CachedReplicate {
    pub(crate) senders: EntityIndexSet,
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
// - 5) New Replicate is added that removes some senders: we want to despawn the entity on those senders
// - 6) New Replicate is added that adds some senders: we want to spawn the entity on those senders
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
//    - we use CachedReplicate to check which senders were removed and had spawned the entity
// 6) new replicate is added that adds some senders:
//    - we use CachedReplicate to check which senders were added

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
