use crate::components::ComponentReplicationOverrides;
use crate::send::sender::ReplicationSender;
use alloc::vec::Vec;
use bevy_ecs::component::{Component, HookContext};
use bevy_ecs::entity::index_set::EntityIndexSet;
use bevy_ecs::observer::Trigger;
use bevy_ecs::prelude::*;
use bevy_ecs::reflect::ReflectComponent;
use bevy_ecs::world::DeferredWorld;
use bevy_reflect::Reflect;
use bevy_time::{Timer, TimerMode};
use lightyear_connection::client::{Client, Connected};
use lightyear_connection::host::HostClient;
use lightyear_connection::network_target::NetworkTarget;
#[cfg(feature = "server")]
use lightyear_connection::{client::PeerMetadata, client_of::ClientOf};
use lightyear_core::id::RemoteId;
use lightyear_link::server::LinkOf;
#[cfg(feature = "server")]
use lightyear_link::server::Server;
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use serde::{Deserialize, Serialize};
#[cfg(any(feature = "client", feature = "server"))]
use tracing::debug;
use tracing::{error, trace};

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

    pub fn enable_all(mut self, sender: Entity) -> Self {
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

#[cfg(feature = "prediction")]
/// Marker component that tells the client to spawn a Predicted entity
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct ShouldBePredicted;

#[cfg(feature = "prediction")]
pub type PredictionTarget = ReplicationTarget<ShouldBePredicted>;

#[cfg(feature = "prediction")]
impl PredictionTarget {
    pub fn add_replication_group(
        trigger: Trigger<OnAdd, PredictionTarget>,
        mut commands: Commands,
    ) {
        // note: we don't need to handle this for ReplicateLike entities because they take the ReplicationGroup from the root entity
        commands.entity(trigger.target()).insert(PREDICTION_GROUP);
    }
}

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
                        error!("No ReplicationSender found in the world");
                        return;
                    };
                    replicate.senders.insert(sender_entity);
                }
                #[cfg(feature = "client")]
                ReplicationMode::SingleClient => {
                    use lightyear_connection::client::Client;
                    use lightyear_connection::host::HostClient;
                    use tracing::{debug, error};
                    let Ok(sender_entity) = world
                        .query_filtered::<Entity, (With<Client>, Or<(With<ReplicationSender>, With<HostClient>)>)>()
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
                                .query_filtered::<(), (With<ClientOf>, Or<(With<ReplicationSender>, With<HostClient>)>)>()
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
                            .query_filtered::<(), Or<(With<ReplicationSender>, With<HostClient>)>>()
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

    pub(crate) fn on_replace(mut world: DeferredWorld, context: HookContext) {
        // TODO: maybe we can just use the CachedReplicate?
        // i.e. if you remove 2 clients from Replicate, than in PreBuffer, we will do the diff
        // and remove those clients from sender.replicated_entities and send the despawn

        let mut replicate = world
            .get_mut::<ReplicationTarget<T>>(context.entity)
            .unwrap();
        replicate.senders = EntityIndexSet::default();
    }

    /// When a new client connects, check if we need to replicate existing entities to it
    pub(crate) fn handle_connection(
        trigger: Trigger<OnAdd, (Connected, ReplicationSender)>,
        mut sender_query: Query<
            (Entity, &mut ReplicationSender, &RemoteId, Option<&LinkOf>),
            With<Connected>,
        >,
        mut replicate_query: Query<(Entity, &mut ReplicationTarget<T>)>,
    ) {
        if let Ok((sender_entity, mut sender, remote_peer_id, link_of)) =
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
                        if link_of.is_some() && target.targets(remote_peer_id) {
                            debug!("Replicating existing entity {entity:?} to newly connected sender {sender_entity:?}");
                            sender.add_replicated_entity(entity, true);
                            replicate.senders.insert(sender_entity);
                        }
                    }
                    ReplicationMode::Sender(_) => {}
                    #[cfg(feature = "server")]
                    ReplicationMode::Server(e, target) => {
                        if target.targets(remote_peer_id) && link_of.is_some_and(|c| c.server == *e) {
                            sender.add_replicated_entity(entity, true);
                            replicate.senders.insert(sender_entity);
                        }
                    }
                    ReplicationMode::Target(target) => {
                        if target.targets(remote_peer_id) {
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
    mode: ReplicationMode,
    #[reflect(ignore)]
    pub senders: EntityIndexSet,
    // do we have authority over this entity?
    authority: bool,
}

impl Replicate {
    pub fn new(mode: ReplicationMode) -> Self {
        Self {
            mode,
            senders: EntityIndexSet::default(),
            authority: true,
        }
    }

    pub fn without_authority(mut self) -> Self {
        self.authority = false;
        self
    }

    #[cfg(feature = "client")]
    pub fn to_server() -> Self {
        Self {
            mode: ReplicationMode::SingleClient,
            senders: EntityIndexSet::default(),
            authority: true,
        }
    }

    #[cfg(feature = "server")]
    pub fn to_clients(target: NetworkTarget) -> Self {
        Self {
            mode: ReplicationMode::SingleServer(target),
            senders: EntityIndexSet::default(),
            authority: true,
        }
    }

    pub fn manual(senders: Vec<Entity>) -> Self {
        Self {
            mode: ReplicationMode::Manual(senders),
            senders: EntityIndexSet::default(),
            authority: true,
        }
    }

    /// List of [`ReplicationSender`] entities that this entity is being replicated on
    pub fn senders(&self) -> impl Iterator<Item = Entity> {
        self.senders.iter().copied()
    }

    fn on_insert(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let unsafe_world = world.as_unsafe_world_cell();
            // SAFETY: we will use this world to access the ReplicationSender
            let world = unsafe { unsafe_world.world_mut() };
            // SAFETY: we will use this world only to access the Replicated entity, so there is no aliasing issue
            let mut replicate_entity_mut =
                unsafe { unsafe_world.world_mut().entity_mut(context.entity) };

            let mut replicate = replicate_entity_mut.get_mut::<Replicate>().unwrap();

            // enable split borrows
            let replicate = &mut *replicate;
            match &mut replicate.mode {
                ReplicationMode::SingleSender => {
                    let Ok((sender_entity, sender, host_client)) = world
                        .query_filtered::<(Entity, Option<&mut ReplicationSender>, Has<HostClient>), Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .single_mut(world)
                    else {
                        error!("No ReplicationSender found in the world");
                        return;
                    };
                    replicate.senders.insert(sender_entity);
                    if host_client {
                        return;
                    }
                    let Some(mut sender) = sender else {
                        error!("No ReplicationSender found in the world");
                        return;
                    };
                    sender.add_replicated_entity(context.entity, replicate.authority);
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
                        error!("No Client found in the world");
                        return;
                    };
                    debug!(
                        "Adding replicated entity {} to sender {}",
                        context.entity, sender_entity
                    );
                    replicate.senders.insert(sender_entity);
                    if host_client {
                        return;
                    }
                    let Some(mut sender) = sender else {
                        error!("No ReplicationSender found in the world");
                        return;
                    };
                    sender.add_replicated_entity(context.entity, replicate.authority);
                }
                #[cfg(feature = "server")]
                ReplicationMode::SingleServer(target) => {
                    use lightyear_connection::client_of::ClientOf;
                    use lightyear_connection::host::HostClient;
                    use lightyear_connection::server::Started;
                    use lightyear_link::server::Server;
                    use tracing::{debug, error, trace};
                    let unsafe_world = world.as_unsafe_world_cell();
                    // SAFETY: we will use this to access the server-entity, which does not alias with the ReplicationSenders
                    let world = unsafe { unsafe_world.world_mut() };
                    let Ok(server) = world
                        .query_filtered::<&Server, With<Started>>()
                        .single(world)
                    else {
                        error!("No Server found in the world");
                        return;
                    };
                    // SAFETY: we will use this to access the PeerMetadata, which does not alias with the ReplicationSenders
                    let world = unsafe { unsafe_world.world_mut() };
                    let peer_metadata =
                        world.resource::<lightyear_connection::client::PeerMetadata>();
                    let world = unsafe { unsafe_world.world_mut() };
                    target.apply_targets(
                        server.collection().iter().copied(),
                        &peer_metadata.mapping,
                        &mut |client| {
                            trace!(
                                "Adding replicated entity {} to ClientOf {}",
                                context.entity, client
                            );
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
                            replicate.senders.insert(client);
                            if host_client {
                                return;
                            }
                            let Some(mut sender) = sender else {
                                error!("No ReplicationSender found in the world");
                                return;
                            };
                            sender.add_replicated_entity(context.entity, replicate.authority);
                        },
                    );
                }
                ReplicationMode::Sender(entity) => {
                    let Ok((sender, host_client)) = world
                        .query_filtered::<(Option<&mut ReplicationSender>, Has<HostClient>), Or<(With<ReplicationSender>, With<HostClient>)>>()
                        .get_mut(world, *entity)
                    else {
                        error!("No ReplicationSender found in the world");
                        return;
                    };
                    replicate.senders.insert(*entity);
                    if host_client {
                        return;
                    }
                    let Some(mut sender) = sender else {
                        error!("No ReplicationSender found in the world");
                        return;
                    };
                    sender.add_replicated_entity(context.entity, replicate.authority);
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
                        .resource::<lightyear_connection::client::PeerMetadata>();
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
                            replicate.senders.insert(client);
                            if host_client {
                                return;
                            }
                            let Some(mut sender) = sender else {
                                error!("No ReplicationSender found in the world");
                                return;
                            };
                            sender.add_replicated_entity(context.entity, replicate.authority);

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
                            error!("No ReplicationSender found in the world");
                            return;
                        };
                        replicate.senders.insert(*entity);
                        if host_client {
                            return;
                        }
                        let Some(mut sender) = sender else {
                            error!("No ReplicationSender found in the world");
                            return;
                        };
                        sender.add_replicated_entity(context.entity, replicate.authority);
                    }
                }
            }
        });
    }

    // think of this as on_drop
    fn on_replace(mut world: DeferredWorld, context: HookContext) {
        // TODO: maybe we can just use the CachedReplicate?
        // i.e. if you remove 2 clients from Replicate, than in PreBuffer, we will do the diff
        // and remove those clients from sender.replicated_entities and send the despawn

        let mut replicate = world.get_mut::<Replicate>(context.entity).unwrap();
        core::mem::take(&mut replicate.senders)
            .iter()
            .for_each(|sender_entity| {
                if let Some(mut sender) = world.get_mut::<ReplicationSender>(*sender_entity) {
                    sender.replicated_entities.swap_remove(&context.entity);
                }
            });
    }

    /// When a new client connects, check if we need to replicate existing entities to it
    pub fn handle_connection(
        trigger: Trigger<OnAdd, (Connected, ReplicationSender)>,
        mut sender_query: Query<
            (
                Entity,
                &mut ReplicationSender,
                &RemoteId,
                Option<&Client>,
                Option<&LinkOf>,
            ),
            With<Connected>,
        >,
        mut replicate_query: Query<(Entity, &mut Replicate)>,
    ) {
        if let Ok((sender_entity, mut sender, remote_peer_id, _client, client_of)) =
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
                        if client_of.is_some() && target.targets(remote_peer_id) {
                            debug!("Replicating existing entity {entity:?} to newly connected sender {sender_entity:?}");
                            sender.add_replicated_entity(entity, replicate.authority);
                            replicate.senders.insert(sender_entity);
                        }
                    }
                    ReplicationMode::Sender(_) => {}
                    #[cfg(feature = "server")]
                    ReplicationMode::Server(e, target) => {
                        if client_of.is_some_and(|c| c.server == *e) && target.targets(remote_peer_id) {
                            sender.add_replicated_entity(entity, replicate.authority);
                            replicate.senders.insert(sender_entity);
                        }
                    }
                    ReplicationMode::Target(target) => {
                        if target.targets(remote_peer_id) {
                            sender.add_replicated_entity(entity, replicate.authority);
                            replicate.senders.insert(sender_entity);
                        }
                    }
                    ReplicationMode::Manual(_) => {}
                }
            })
        }
    }
}

#[derive(Component, Debug)]
pub struct CachedReplicate {
    pub(crate) senders: EntityIndexSet,
}
