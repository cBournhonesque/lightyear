use alloc::vec::Vec;
use bevy_app::App;
use bevy_ecs::prelude::*;
use bevy_ecs::relationship::Relationship;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::prelude::*;
use bevy_replicon::shared::replication::registry::ctx::{SerializeCtx, WriteCtx};
use bevy_replicon::shared::server_entity_map::ServerEntityMap;
#[cfg(feature = "client")]
use {
    bevy_enhanced_input::context::ExternallyMocked,
    lightyear_connection::client::Client,
    lightyear_replication::prelude::{Controlled, Replicate},
};

use bevy_enhanced_input::prelude::*;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_utils::prelude::DebugName;
#[cfg(all(feature = "client", feature = "server"))]
use lightyear_connection::host::HostServer;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::PreSpawned;
#[allow(unused_imports)]
use tracing::{debug, info};
#[cfg(any(feature = "client", feature = "server"))]
use {
    lightyear_connection::{host::HostClient, server::Started},
    lightyear_link::prelude::Server,
};
#[cfg(feature = "server")]
use {
    lightyear_inputs::server::ServerInputConfig,
    lightyear_replication::prelude::{InterpolationTarget, PredictionTarget, ReplicateLike},
};
// TODO: ideally we would have an entity-mapped that is PreSpawn aware. If you include an entity
//   that is PreSpawned, then in the entity-mapper we use a Query<Entity, With<PreSpawned>> to check the hash
//   of the entity and serialize it as the hash. Then the receiving entity mapper could look up the corresponding
//   entity by the PreSpawn hash to apply entity mapping.
//   1. In common case, server sends P1,C1. It does NOT need to change ChildOf(P1) because client will match P1/C1 on receipt, then
//        update its entity maps, then the component map entity will work correctly. We just need to make sure that C1 is also Prespawned,
//        which we could do in ReplicateLike Propagation? (but how to do it on the receiver side?)
//

pub struct InputRegistryPlugin;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NetworkActionOf<C> {
    entity: Entity,
    marker: core::marker::PhantomData<C>,
}

impl<C> NetworkActionOf<C> {
    fn new(entity: Entity) -> Self {
        Self {
            entity,
            marker: core::marker::PhantomData,
        }
    }

    fn get(&self) -> Entity {
        self.entity
    }
}

impl InputRegistryPlugin {
    pub(crate) fn mirror_action_of_for_replication<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        action_of: Query<&ActionOf<C>, Without<Remote>>,
        remote_contexts: Query<(), With<Remote>>,
        entity_map: Option<Res<ServerEntityMap>>,
        managers: Query<&MessageManager>,
        mut commands: Commands,
    ) {
        let entity = trigger.entity;
        let Ok(action_of) = action_of.get(entity) else {
            return;
        };

        let context_entity = action_of.get();
        let remote_entity = resolve_remote_action_context(
            context_entity,
            remote_contexts.contains(context_entity),
            entity_map.as_deref(),
            managers.iter(),
        );
        let Some(remote_entity) = remote_entity else {
            return;
        };
        commands
            .entity(entity)
            .insert(NetworkActionOf::<C>::new(remote_entity));
    }

    pub(crate) fn resolve_pending_network_action_of<C: Component>(
        pending: Query<(Entity, &ActionOf<C>), (Without<NetworkActionOf<C>>, Without<Remote>)>,
        remote_contexts: Query<(), With<Remote>>,
        entity_map: Option<Res<ServerEntityMap>>,
        managers: Query<&MessageManager>,
        mut commands: Commands,
    ) {
        for (entity, action_of) in pending.iter() {
            let context_entity = action_of.get();
            let remote_entity = resolve_remote_action_context(
                context_entity,
                remote_contexts.contains(context_entity),
                entity_map.as_deref(),
                managers.iter(),
            );
            let Some(remote_entity) = remote_entity else {
                continue;
            };

            commands
                .entity(entity)
                .insert(NetworkActionOf::<C>::new(remote_entity));
        }
    }

    pub(crate) fn insert_action_of_from_network<C: Component>(
        trigger: On<Add, NetworkActionOf<C>>,
        query: Query<&NetworkActionOf<C>, (Without<ActionOf<C>>, With<Remote>)>,
        entity_map: Option<Res<ServerEntityMap>>,
        managers: Query<&MessageManager>,
        all_entities: Query<(), ()>,
        mut commands: Commands,
    ) {
        let entity = trigger.entity;
        let Ok(network_action_of) = query.get(entity) else {
            return;
        };

        if let Some(mapped) = resolve_local_entity(
            network_action_of.get(),
            entity_map.as_deref(),
            managers.iter(),
            &all_entities,
        ) {
            commands.entity(entity).insert(ActionOf::<C>::new(mapped));
        }
    }

    pub(crate) fn resolve_pending_action_of<C: Component>(
        pending: Query<(Entity, &NetworkActionOf<C>), (Without<ActionOf<C>>, With<Remote>)>,
        entity_map: Option<Res<ServerEntityMap>>,
        managers: Query<&MessageManager>,
        all_entities: Query<(), ()>,
        mut commands: Commands,
    ) {
        for (entity, network_action_of) in pending.iter() {
            if let Some(mapped) = resolve_local_entity(
                network_action_of.get(),
                entity_map.as_deref(),
                managers.iter(),
                &all_entities,
            ) {
                commands.entity(entity).insert(ActionOf::<C>::new(mapped));
            }
        }
    }

    /// For Host-Server, if an ActionOf is spawned directly on the HostClient.
    /// (without being received from replication, or with Prespawned)
    /// Then we initiate rebroadcast
    #[cfg(all(feature = "client", feature = "server"))]
    pub(crate) fn add_action_of_host_server_rebroadcast<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        host_server: Single<(), With<HostServer>>,
        action: Query<&ActionOf<C>, Or<(Without<Remote>, With<PreSpawned>)>>,
        mut commands: Commands,
    ) {
        let entity = trigger.entity;
        if let Ok(action_of) = action.get(entity) {
            let context_entity = action_of.get();
            debug!(action_entity = ?entity, "Replicating ActionOf<{:?}> for context entity {context_entity:?} from HostClient to other clients for input rebroadcast", DebugName::type_name::<C>());
            commands.entity(entity).insert((ReplicateLike {
                root: context_entity,
            },));
        }
    }

    /// When an [`ActionOf<C>`] component is added to an entity (usually on the client),
    /// we add Replicate to it so that the action entity is also created on the server.
    ///
    /// PreSpawned Actions must be replicated from server to client.
    /// No need to change anything about ActionOf because the Context and Action will be received at the same time,
    /// so the entity mapping in ActionOf will work properly.
    #[cfg(feature = "client")]
    pub(crate) fn add_action_of_replicate<C: Component>(
        trigger: On<Add, NetworkActionOf<C>>,
        server: Query<(), (With<Server>, With<Started>)>,
        // we don't want to add Replicate on action entities that were already received
        // PreSpawned entities are replicated from server to client
        action: Query<
            &ActionOf<C>,
            (
                With<NetworkActionOf<C>>,
                Without<Remote>,
                Without<PreSpawned>,
            ),
        >,
        mut commands: Commands,
    ) {
        if server.single().is_ok() {
            // we're on the server, don't do anything
            return;
        }
        let entity = trigger.entity;
        if let Ok(action_of) = action.get(entity) {
            let context_entity = action_of.get();
            debug!(action_entity = ?entity, "Replicating ActionOf<{:?}> for context entity {context_entity:?} from client to server", DebugName::type_name::<C>());
            commands.entity(entity).insert((Replicate::to_server(),));
        }
    }

    /// When the server receives [`ActionOf`], optionally rebroadcast to other clients if rebroadcast_inputs is enabled
    #[cfg(feature = "server")]
    pub(crate) fn on_action_of_replicated<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        query: Query<&ActionOf<C>, With<Remote>>,
        mut host: Query<&mut MessageManager, With<HostClient>>,
        _: Single<(), (With<Server>, With<Started>)>,
        config: Res<ServerInputConfig<C>>,
        mut commands: Commands,
    ) {
        let entity = trigger.entity;
        if let Ok(wrapper) = query.get(entity) {
            debug!(?entity, context = ?DebugName::type_name::<C>(), "Server received action entity");

            // If rebroadcast_inputs is enabled, set up replication to other clients
            if config.rebroadcast_inputs {
                debug!(action_entity = ?entity, "On server, rebroadcast by inserting ReplicateLike({:?}) for action entity ActionOf<{:?}>", wrapper.get(), DebugName::type_name::<C>());

                // TODO: don't rebroadcast to the original client
                commands.entity(entity).insert((
                    ReplicateLike {
                        root: wrapper.get(),
                    },
                    // we don't want to spawn Predicted Action entities
                    PredictionTarget::manual(alloc::vec![]),
                    InterpolationTarget::manual(alloc::vec![]),
                ));

                // This is subtle. The client-of receives the entity, and will try to rebroadcast input messages
                // to other clients. But the host-server client won't apply entity-mapping correctly for that
                // action entity because it doesn't receive replication messages, so its entity map is empty!
                // A long-term solution might be to have the HostClient contain EVERY replicated entity in its
                // entity-map, but for now let's just add the action entity
                if let Ok(mut message_manager) = host.single_mut() {
                    message_manager.entity_mapper.insert(entity, entity);
                }
            }
        }
    }

    /// When the client receives a rebroadcast Action entity with [`Remote`],
    ///
    /// Attach ExternallyMocked to it to signify that the ActionState should only be updated
    /// from rebroadcasted input messages. (in particular, BEI doesn't tick the time for those actions)
    #[cfg(feature = "client")]
    pub(crate) fn on_rebroadcast_action_received<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        single: Single<(), (With<Client>, Without<HostClient>)>,
        query: Query<&ActionOf<C>, With<Remote>>,
        controlled: Query<(), With<Controlled>>,
        mut commands: Commands,
    ) {
        if let Ok(action_of) = query.get(trigger.entity) {
            if controlled.contains(action_of.get()) {
                return;
            }
            let entity = trigger.entity;
            debug!(
                ?entity,
                "On client, received ActionOf({:?}) for action entity ActionOf<{:?}> from input rebroadcast",
                action_of.get(),
                DebugName::type_name::<C>()
            );

            commands.entity(entity).insert(
                // Make sure that the actions are only updated via input messages
                ExternallyMocked,
            );
        }
    }
}

fn resolve_remote_entity<'a>(
    local_entity: Entity,
    entity_map: Option<&ServerEntityMap>,
    mut managers: impl Iterator<Item = &'a MessageManager>,
) -> Option<Entity> {
    if let Some(entity_map) = entity_map
        && let Some(remote_entity) = entity_map.to_server().get(&local_entity)
    {
        return Some(*remote_entity);
    }

    managers.find_map(|manager| manager.entity_mapper.get_remote(local_entity))
}

fn resolve_remote_action_context<'a>(
    local_entity: Entity,
    remote_context: bool,
    entity_map: Option<&ServerEntityMap>,
    managers: impl Iterator<Item = &'a MessageManager>,
) -> Option<Entity> {
    resolve_remote_entity(local_entity, entity_map, managers)
        .or_else(|| (!remote_context).then_some(local_entity))
}

fn resolve_local_entity<'a>(
    remote_entity: Entity,
    entity_map: Option<&ServerEntityMap>,
    mut managers: impl Iterator<Item = &'a MessageManager>,
    all_entities: &Query<(), ()>,
) -> Option<Entity> {
    if let Some(entity_map) = entity_map
        && let Some(local_entity) = entity_map.to_client().get(&remote_entity)
    {
        return Some(*local_entity);
    }

    if let Some(local_entity) =
        managers.find_map(|manager| manager.entity_mapper.get_local(remote_entity))
    {
        return Some(local_entity);
    }

    all_entities.get(remote_entity).ok().map(|()| remote_entity)
}

// we don't care about the actual data in Action<A>, so nothing to serialize
fn serialize_action<A: InputAction>(
    _ctx: &SerializeCtx,
    _: &Action<A>,
    _: &mut Vec<u8>,
) -> bevy_ecs::error::Result<()> {
    Ok(())
}
fn deserialize_action<A: InputAction>(
    _: &mut WriteCtx,
    _: &mut Bytes,
) -> bevy_ecs::error::Result<Action<A>> {
    Ok(Action::<A>::default())
}

/// Serialize the authoritative remote entity for an action context.
///
/// Entity mapping is handled out-of-band before replication by mirroring [`ActionOf<C>`]
/// into [`NetworkActionOf<C>`].
pub(crate) fn serialize_network_action_of<C: Component>(
    _ctx: &SerializeCtx,
    action_of: &NetworkActionOf<C>,
    message: &mut Vec<u8>,
) -> bevy_ecs::error::Result<()> {
    bevy_replicon::postcard_utils::entity_to_extend_mut(&action_of.get(), message)?;
    Ok(())
}

/// Deserialize the authoritative remote entity for an action context.
///
/// We intentionally do not apply replicon's entity mapping here because the authoritative
/// entity may come from either the replicon server map or lightyear's message entity map.
pub(crate) fn deserialize_network_action_of<C: Component>(
    _: &mut WriteCtx,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<NetworkActionOf<C>> {
    let entity = bevy_replicon::postcard_utils::entity_from_buf(message)?;
    Ok(NetworkActionOf::<C>::new(entity))
}

pub trait InputRegistryExt {
    /// Registers a new input action type and returns its kind.
    fn register_input_action<A: InputAction>(self) -> Self;
}

impl InputRegistryExt for &mut App {
    fn register_input_action<A: InputAction>(self) -> Self {
        self.replicate_with((
            RuleFns::new(serialize_action::<A>, deserialize_action::<A>),
            ReplicationMode::Once,
        ));
        self
    }
}
