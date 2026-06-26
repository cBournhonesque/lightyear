use alloc::vec::Vec;
use bevy_app::App;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_ecs::prelude::*;
use bevy_ecs::relationship::Relationship;
use bevy_replicon::prelude::*;
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, SerializeCtx, WriteCtx};
#[cfg(feature = "client")]
use bevy_replicon::shared::server_entity_map::ServerEntityMap;
use bevy_replicon::{bytes::Bytes, postcard_utils};
#[cfg(feature = "client")]
use {
    bevy_enhanced_input::context::ExternallyMocked,
    lightyear_connection::client::Client,
    lightyear_replication::prelude::{Controlled, ControlledBy},
};

use bevy_enhanced_input::prelude::*;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_utils::prelude::DebugName;
#[cfg(any(feature = "client", feature = "server"))]
use lightyear_connection::host::HostClient;
#[cfg(all(feature = "client", feature = "server"))]
use lightyear_connection::host::HostServer;
#[cfg(feature = "server")]
use lightyear_connection::server::Started;
#[cfg(feature = "server")]
use lightyear_link::prelude::Server;
#[cfg(feature = "server")]
use lightyear_messages::MessageManager;
#[cfg(all(feature = "client", feature = "server"))]
use lightyear_replication::prelude::PreSpawned;
#[allow(unused_imports)]
use tracing::{debug, warn};
#[cfg(feature = "server")]
use {
    lightyear_inputs::server::ServerInputConfig,
    lightyear_replication::prelude::{InterpolationTarget, PredictionTarget, ReplicateLike},
};

/// Client-side placeholder for a replicated [`ActionOf<C>`] whose context
/// entity has not been mapped yet.
///
/// This is deliberately local-only. Once the context entity appears in
/// Replicon's server-to-client entity map, [`resolve_pending_action_of`]
/// replaces this component with the real BEI relationship.
#[derive(Component)]
pub(crate) struct PendingActionOf<C: Component> {
    server_context: Entity,
    marker: core::marker::PhantomData<C>,
}

impl<C: Component> PendingActionOf<C> {
    fn new(server_context: Entity) -> Self {
        Self {
            server_context,
            marker: core::marker::PhantomData,
        }
    }
}

pub struct InputRegistryPlugin;

impl InputRegistryPlugin {
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

    /// In host-server mode, server-owned action entities for remote clients can
    /// still carry keyboard bindings because the authoritative server world and
    /// local host client share one Bevy app. Those actions must be driven by
    /// received input messages, not by the host player's physical keyboard.
    #[cfg(all(feature = "client", feature = "server"))]
    pub(crate) fn mock_non_host_owned_action<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        host_server: Query<(), With<HostServer>>,
        action: Query<&ActionOf<C>, Without<ExternallyMocked>>,
        controlled: Query<&ControlledBy>,
        host_clients: Query<(), With<HostClient>>,
        mut commands: Commands,
    ) {
        if host_server.is_empty() {
            return;
        }
        let entity = trigger.entity;
        let Ok(action_of) = action.get(entity) else {
            return;
        };
        let Ok(controlled_by) = controlled.get(action_of.get()) else {
            return;
        };
        if host_clients.get(controlled_by.owner).is_ok() {
            return;
        }
        commands.entity(entity).insert(ExternallyMocked);
    }

    #[cfg(all(feature = "client", feature = "server"))]
    pub(crate) fn mock_non_host_owned_actions_on_controlled_by<C: Component>(
        trigger: On<Add, ControlledBy>,
        host_server: Query<(), With<HostServer>>,
        controlled: Query<&ControlledBy>,
        host_clients: Query<(), With<HostClient>>,
        actions: Query<(Entity, &ActionOf<C>), Without<ExternallyMocked>>,
        mut commands: Commands,
    ) {
        if host_server.is_empty() {
            return;
        }
        let Ok(controlled_by) = controlled.get(trigger.entity) else {
            return;
        };
        if host_clients.get(controlled_by.owner).is_ok() {
            return;
        }
        for (action_entity, action_of) in &actions {
            if action_of.get() == trigger.entity {
                commands.entity(action_entity).insert(ExternallyMocked);
            }
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

/// Serializes the server context entity targeted by [`ActionOf<C>`].
///
/// This uses a custom rule instead of Replicon's default component
/// serialization because [`ActionOf<C>`] is a relationship component. The
/// default receive path would call [`EntityMapper::get_mapped`] for the context
/// entity and create a placeholder if the context has not been mapped yet. That
/// placeholder is unsafe for a relationship component: Bevy relationship hooks
/// can observe the target during insertion, and Replicon also asserts if that
/// buffered placeholder is still pending when the next component in the same
/// entity bundle is decoded.
pub(crate) fn serialize_action_of<C: Component>(
    _ctx: &SerializeCtx,
    action_of: &ActionOf<C>,
    message: &mut Vec<u8>,
) -> bevy_ecs::error::Result<()> {
    postcard_utils::entity_to_extend_mut(&action_of.get(), message)?;
    Ok(())
}

/// Deserializes the raw server context entity for stale-message consumption.
///
/// The active receive path uses [`write_action_of`] below. This function exists
/// for Replicon's `RuleFns` contract and for consuming stale updates without
/// creating mapped placeholder entities.
pub(crate) fn deserialize_action_of<C: Component>(
    _ctx: &mut WriteCtx,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<ActionOf<C>> {
    let server_context = postcard_utils::entity_from_buf(message)?;
    Ok(ActionOf::new(server_context))
}

/// Receives [`ActionOf<C>`] without using Replicon's placeholder entity mapper.
///
/// If the context entity is already mapped, this inserts the real BEI
/// relationship immediately. If not, it stores [`PendingActionOf<C>`] so the
/// relationship can be attached later by [`resolve_pending_action_of`].
pub(crate) fn write_action_of<C: Component>(
    ctx: &mut WriteCtx,
    _rule_fns: &RuleFns<ActionOf<C>>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<()> {
    let server_context = postcard_utils::entity_from_buf(message)?;
    if let Some(&client_context) = ctx.entity_map.to_client().get(&server_context) {
        entity.insert(ActionOf::<C>::new(client_context));
        entity.remove::<PendingActionOf<C>>();
    } else {
        entity.insert(PendingActionOf::<C>::new(server_context));
        entity.remove::<ActionOf<C>>();
    }
    Ok(())
}

pub(crate) fn remove_action_of<C: Component>(_ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
    entity.remove::<ActionOf<C>>();
    entity.remove::<PendingActionOf<C>>();
}

/// Attach delayed BEI relationships once Replicon has mapped their context.
#[cfg(feature = "client")]
pub(crate) fn resolve_pending_action_of<C: Component>(
    entity_map: Option<Res<ServerEntityMap>>,
    pending: Query<(Entity, &PendingActionOf<C>)>,
    mut commands: Commands,
) {
    let Some(entity_map) = entity_map else {
        return;
    };
    for (entity, pending) in &pending {
        let Some(&client_context) = entity_map.to_client().get(&pending.server_context) else {
            continue;
        };
        commands
            .entity(entity)
            .insert(ActionOf::<C>::new(client_context))
            .remove::<PendingActionOf<C>>();
    }
}

/// Serializes only the presence and type of [`Action<A>`].
///
/// The value stored inside BEI's [`Action<A>`] is local runtime state. Lightyear
/// sends that state through [`BEIStateSequence`](crate::input_message::BEIStateSequence),
/// whose snapshots include the trigger state, action value, events, and timing.
/// The [`Action<A>`] component also does not carry the context relationship:
/// [`ActionOf<C>`] is replicated as its own component, and Replicon's default
/// mapping path is avoided there because it can create placeholder relationship
/// targets when the context has not been mapped yet. Instead, Lightyear defers
/// inserting [`ActionOf<C>`] until the context entity is present in the
/// server-to-client map. Therefore component replication only needs to create
/// the correctly typed action component on the receiver, and
/// [`deserialize_action`] can rebuild it from `Default`.
///
/// [`ActionOf<C>`]: bevy_enhanced_input::prelude::ActionOf
fn serialize_action<A: InputAction>(
    _ctx: &mut SerializeCtx,
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
