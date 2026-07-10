#[cfg(feature = "client")]
use crate::marker::InputMarker;
use alloc::vec::Vec;
use bevy_app::App;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_ecs::prelude::*;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_ecs::relationship::Relationship;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::prelude::*;
use bevy_replicon::shared::replication::registry::ctx::{SerializeCtx, WriteCtx};
#[cfg(all(feature = "client", feature = "server"))]
use lightyear_replication::prelude::ControlledBy;
#[cfg(feature = "client")]
use {
    bevy_enhanced_input::context::ExternallyMocked, lightyear_connection::client::Client,
    lightyear_replication::prelude::Controlled,
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

pub struct InputRegistryPlugin;

impl InputRegistryPlugin {
    /// For Host-Server, if an ActionOf is spawned directly on the HostClient.
    /// (without being received from replication, or with Prespawned)
    /// Then we initiate rebroadcast
    #[cfg(all(feature = "client", feature = "server"))]
    pub(crate) fn add_action_of_host_server_rebroadcast<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        host_server: Query<(), With<HostServer>>,
        action: Query<&ActionOf<C>, Or<(Without<Remote>, With<PreSpawned>)>>,
        mut commands: Commands,
    ) {
        if host_server.is_empty() {
            return;
        }
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
        commands
            .entity(entity)
            .remove::<(Bindings, InputMarker<C>)>()
            .insert(ExternallyMocked);
    }

    #[cfg(all(feature = "client", feature = "server"))]
    pub(crate) fn mock_non_host_owned_actions_on_controlled_by<C: Component>(
        trigger: On<Add, ControlledBy>,
        host_server: Query<(), With<HostServer>>,
        context: Query<(&Actions<C>, &ControlledBy)>,
        host_clients: Query<(), With<HostClient>>,
        mut commands: Commands,
    ) {
        if host_server.is_empty() {
            return;
        }
        let Ok((actions, controlled_by)) = context.get(trigger.entity) else {
            return;
        };
        if host_clients.get(controlled_by.owner).is_ok() {
            return;
        }
        for action_entity in actions.iter() {
            commands
                .entity(action_entity)
                .remove::<(Bindings, InputMarker<C>)>()
                .insert(ExternallyMocked);
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

    /// When a remote ActionOf arrives, update its mocked state if its context
    /// is already present. If the context has not arrived yet, a context
    /// observer will handle it later.
    #[cfg(feature = "client")]
    pub(crate) fn on_rebroadcast_action_received<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        clients: Query<(), (With<Client>, Without<HostClient>)>,
        actions: Query<(&ActionOf<C>, Has<ExternallyMocked>, Has<Bindings>), With<Remote>>,
        contexts: Query<(), With<C>>,
        controlled: Query<(), With<Controlled>>,
        mut commands: Commands,
    ) {
        if clients.is_empty() {
            return;
        }
        let entity = trigger.entity;
        let Ok((action_of, is_mocked, has_bindings)) = actions.get(entity) else {
            return;
        };
        update_rebroadcast_action_mocking::<C>(
            entity,
            is_mocked,
            has_bindings,
            contexts.contains(action_of.get()),
            controlled.contains(action_of.get()),
            &mut commands,
        );
    }

    /// When the context arrives after one or more remote action entities, mock
    /// or unmock those actions now that the control state can be inspected.
    #[cfg(feature = "client")]
    pub(crate) fn on_rebroadcast_context_received<C: Component>(
        trigger: On<Add, C>,
        clients: Query<(), (With<Client>, Without<HostClient>)>,
        contexts: Query<(&Actions<C>, Has<Controlled>), With<C>>,
        actions: Query<(&ActionOf<C>, Has<ExternallyMocked>, Has<Bindings>), With<Remote>>,
        mut commands: Commands,
    ) {
        if clients.is_empty() {
            return;
        }
        let Ok((context_actions, has_controlled)) = contexts.get(trigger.entity) else {
            return;
        };
        update_rebroadcast_actions_for_context::<C>(
            context_actions,
            has_controlled,
            &actions,
            &mut commands,
        );
    }

    /// When Controlled arrives after a remote context/action relationship,
    /// remove mocking from locally controlled actions.
    #[cfg(feature = "client")]
    pub(crate) fn on_rebroadcast_context_controlled<C: Component>(
        trigger: On<Add, Controlled>,
        clients: Query<(), (With<Client>, Without<HostClient>)>,
        contexts: Query<(&Actions<C>, Has<Controlled>), With<C>>,
        actions: Query<(&ActionOf<C>, Has<ExternallyMocked>, Has<Bindings>), With<Remote>>,
        mut commands: Commands,
    ) {
        if clients.is_empty() {
            return;
        }
        let Ok((context_actions, has_controlled)) = contexts.get(trigger.entity) else {
            return;
        };
        update_rebroadcast_actions_for_context::<C>(
            context_actions,
            has_controlled,
            &actions,
            &mut commands,
        );
    }
}

#[cfg(feature = "client")]
fn update_rebroadcast_actions_for_context<C: Component>(
    context_actions: &Actions<C>,
    has_controlled: bool,
    actions: &Query<(&ActionOf<C>, Has<ExternallyMocked>, Has<Bindings>), With<Remote>>,
    commands: &mut Commands,
) {
    for action_entity in context_actions.iter() {
        let Ok((_, is_mocked, has_bindings)) = actions.get(action_entity) else {
            continue;
        };
        update_rebroadcast_action_mocking::<C>(
            action_entity,
            is_mocked,
            has_bindings,
            true,
            has_controlled,
            commands,
        );
    }
}

#[cfg(feature = "client")]
fn update_rebroadcast_action_mocking<C: Component>(
    entity: Entity,
    is_mocked: bool,
    has_bindings: bool,
    context_is_ready: bool,
    context_is_controlled: bool,
    commands: &mut Commands,
) {
    if !context_is_ready {
        return;
    }

    if context_is_controlled {
        if is_mocked {
            let mut entity_commands = commands.entity(entity);
            entity_commands.remove::<ExternallyMocked>();
            if has_bindings {
                entity_commands.insert(InputMarker::<C>::default());
            }
        }
        return;
    }

    if !is_mocked {
        debug!(
            ?entity,
            "On client, mocked remote action entity ActionOf<{:?}> from input rebroadcast",
            DebugName::type_name::<C>()
        );
        commands
            .entity(entity)
            // Make sure that the action is only updated via input messages.
            .remove::<(Bindings, InputMarker<C>)>()
            .insert(ExternallyMocked);
    }
}

/// Serializes only the presence and type of [`Action<A>`].
///
/// The value stored inside BEI's [`Action<A>`] is local runtime state. Lightyear
/// sends that state through [`BEIStateSequence`](crate::input_message::BEIStateSequence),
/// whose snapshots include the trigger state, action value, events, and timing.
/// The [`Action<A>`] component also does not carry the context relationship:
/// [`ActionOf<C>`] is replicated as its own component. Therefore component
/// replication only needs to create the correctly typed action component on the
/// receiver, and [`deserialize_action`] can rebuild it from `Default`.
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

#[cfg(all(test, feature = "client"))]
mod tests {
    use super::*;
    use lightyear_connection::client::Client;
    use lightyear_replication::prelude::Controlled;

    #[derive(Component)]
    struct TestContext;

    fn app_with_rebroadcast_mocking() -> App {
        let mut app = App::new();
        app.add_observer(InputRegistryPlugin::on_rebroadcast_action_received::<TestContext>);
        app.add_observer(InputRegistryPlugin::on_rebroadcast_context_received::<TestContext>);
        app.add_observer(InputRegistryPlugin::on_rebroadcast_context_controlled::<TestContext>);
        app.world_mut().spawn(Client);
        app
    }

    #[test]
    fn controlled_remote_action_is_not_left_externally_mocked() {
        let mut app = app_with_rebroadcast_mocking();
        let context = app.world_mut().spawn((TestContext, Controlled)).id();
        let action = app
            .world_mut()
            .spawn((
                ActionOf::<TestContext>::new(context),
                Remote,
                ExternallyMocked,
                Bindings::default(),
            ))
            .id();

        app.update();

        let action = app.world().entity(action);
        assert!(!action.contains::<ExternallyMocked>());
        assert!(action.contains::<Bindings>());
        assert!(action.contains::<InputMarker<TestContext>>());
    }

    #[test]
    fn uncontrolled_remote_action_is_externally_mocked() {
        let mut app = app_with_rebroadcast_mocking();
        let context = app.world_mut().spawn(TestContext).id();
        let action = app
            .world_mut()
            .spawn((
                ActionOf::<TestContext>::new(context),
                Remote,
                Bindings::default(),
                InputMarker::<TestContext>::default(),
            ))
            .id();

        app.update();

        let action = app.world().entity(action);
        assert!(action.contains::<ExternallyMocked>());
        assert!(!action.contains::<Bindings>());
        assert!(!action.contains::<InputMarker<TestContext>>());
    }

    #[test]
    fn remote_action_waits_for_context_before_mocking() {
        let mut app = app_with_rebroadcast_mocking();
        let context = app.world_mut().spawn_empty().id();
        let action = app
            .world_mut()
            .spawn((
                ActionOf::<TestContext>::new(context),
                Remote,
                Bindings::default(),
                InputMarker::<TestContext>::default(),
            ))
            .id();

        app.update();

        let action_ref = app.world().entity(action);
        assert!(!action_ref.contains::<ExternallyMocked>());
        assert!(action_ref.contains::<Bindings>());
        assert!(action_ref.contains::<InputMarker<TestContext>>());

        app.world_mut().entity_mut(context).insert(TestContext);
        app.update();

        let action_ref = app.world().entity(action);
        assert!(action_ref.contains::<ExternallyMocked>());
        assert!(!action_ref.contains::<Bindings>());
        assert!(!action_ref.contains::<InputMarker<TestContext>>());
    }

    #[test]
    fn controlled_context_added_later_unmocks_remote_action() {
        let mut app = app_with_rebroadcast_mocking();
        let context = app.world_mut().spawn(TestContext).id();
        let action = app
            .world_mut()
            .spawn((
                ActionOf::<TestContext>::new(context),
                Remote,
                Bindings::default(),
                InputMarker::<TestContext>::default(),
            ))
            .id();

        app.update();
        assert!(app.world().entity(action).contains::<ExternallyMocked>());

        app.world_mut().entity_mut(context).insert(Controlled);
        app.update();

        assert!(!app.world().entity(action).contains::<ExternallyMocked>());
    }
}
