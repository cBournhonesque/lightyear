use crate::protocol::{BEIAction1, BEIContext};
use crate::stepper::*;
use bevy::prelude::*;
use bevy_enhanced_input::action::mock::{ActionMock, MockSpan};
use bevy_enhanced_input::action::{Action, TriggerState};
use bevy_enhanced_input::prelude::{ActionOf, ActionValue, Actions};
use lightyear::input::bei::input_message::BEIBuffer;
use lightyear::input::bei::prelude::InputMarker;
use lightyear::input::input_buffer::Compressed;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::{ControlledBy, PreSpawned, PredictionTarget, Replicate};

const TEST_HASH: u64 = 42;

/// Check that a non-host client action is rebroadcast to the other remote client
/// while a host client is present in the topology.
#[test]
fn test_rebroadcast() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::from_link_types(
        vec![ClientType::Host, ClientType::Netcode, ClientType::Netcode],
        ServerType::Netcode,
    ));

    // Entity controlled by client 1, with inputs rebroadcast to all clients
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            BEIContext,
        ))
        .id();

    // Spawn action entity on the server with PreSpawned
    let _server_action = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(server_entity),
            Action::<BEIAction1>::default(),
            PreSpawned::new(TEST_HASH),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();

    stepper.frame_step_server_first(1);

    // Get the predicted entities on both clients
    let client0_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity not replicated to client 1");
    let client1_entity = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity not replicated to client 2");

    // Spawn matching action entity on client 0 with PreSpawned, then let the
    // ActionOf/InputMarker relationship settle before mocking input.
    stepper.client_apps[0]
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(client0_entity),
            Action::<BEIAction1>::default(),
            ActionMock::new(
                TriggerState::Fired,
                ActionValue::Bool(true),
                MockSpan::Manual,
            ),
            PreSpawned::new(TEST_HASH),
            InputMarker::<BEIContext>::default(),
        ))
        .id();
    stepper.frame_step(4);

    // Check that client 1 received the rebroadcasted action and has input buffer
    let action1 =
        find_action_with_fired_input(stepper.client_apps[1].world(), client1_entity, "client 1");
    let remote_buffer = stepper.client_apps[1]
        .world()
        .entity(action1)
        .get::<BEIBuffer<BEIContext>>()
        .expect("Client 1 should have a BEI input buffer for the rebroadcasted action");
    assert_buffer_contains_fired_input(remote_buffer, "client 1 rebroadcast buffer");

    // Verify the server has the action entity
    let action_host = stepper
        .server_app
        .world()
        .get::<Actions<BEIContext>>(server_entity)
        .unwrap()
        .collection()[0];
    assert!(
        stepper
            .server_app
            .world()
            .entity(action_host)
            .contains::<Action<BEIAction1>>(),
        "Action entity on server should have the Action component"
    );
    let server_buffer = stepper
        .server_app
        .world()
        .entity(action_host)
        .get::<BEIBuffer<BEIContext>>()
        .expect("Server should buffer client 0's input before rebroadcasting it");
    assert_buffer_contains_fired_input(server_buffer, "server input buffer");
}

/// Minimal host-server topology: host client fires an action and the remote client
/// receives a rebroadcast action entity with buffered input state.
#[test]
fn test_host_client_action_rebroadcasts_to_remote_client() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::host_server());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: stepper.host_client_entity.unwrap(),
                lifetime: Default::default(),
            },
            BEIContext,
        ))
        .id();

    let server_action = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(server_entity),
            Action::<BEIAction1>::default(),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();

    stepper.frame_step_server_first(1);

    let remote_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity not replicated to remote client");

    // The host client lives in the server app, so driving the server-side action entity
    // exercises the host-client local input path.
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_action)
        .insert(ActionMock::new(
            TriggerState::Fired,
            ActionValue::Bool(true),
            MockSpan::Manual,
        ));

    stepper.frame_step(4);

    let remote_action = find_action_with_fired_input(
        stepper.client_apps[0].world(),
        remote_entity,
        "remote client",
    );
    assert!(
        stepper.client_apps[0]
            .world()
            .entity(remote_action)
            .contains::<Action<BEIAction1>>(),
        "Remote client should receive the rebroadcast action entity"
    );
    let remote_buffer = stepper.client_apps[0]
        .world()
        .entity(remote_action)
        .get::<BEIBuffer<BEIContext>>()
        .expect("Remote client should buffer the host client's rebroadcast input");
    assert_buffer_contains_fired_input(remote_buffer, "remote host-client rebroadcast buffer");

    let server_action = stepper
        .server_app
        .world()
        .get::<Actions<BEIContext>>(server_entity)
        .unwrap()
        .collection()[0];
    assert!(
        stepper
            .server_app
            .world()
            .entity(server_action)
            .contains::<Action<BEIAction1>>(),
        "Server should also have the host action entity"
    );
    let server_buffer = stepper
        .server_app
        .world()
        .entity(server_action)
        .get::<BEIBuffer<BEIContext>>()
        .expect("Server should buffer the host client's input before rebroadcasting it");
    assert_buffer_contains_fired_input(server_buffer, "host-server input buffer");
}

fn assert_buffer_contains_fired_input(buffer: &BEIBuffer<BEIContext>, label: &str) {
    assert!(
        buffer_contains_fired_input(buffer),
        "{label} should contain a fired bool input, got {buffer:?}"
    );
}

fn buffer_contains_fired_input(buffer: &BEIBuffer<BEIContext>) -> bool {
    buffer.buffer.iter().any(|input| {
        matches!(
            input,
            Compressed::Input(snapshot)
                if snapshot.state == TriggerState::Fired
                    && snapshot.value == ActionValue::Bool(true)
        )
    })
}

fn find_action_with_fired_input(world: &World, context: Entity, label: &str) -> Entity {
    let actions = world
        .get::<Actions<BEIContext>>(context)
        .unwrap_or_else(|| panic!("{label} context should have BEI actions"));
    for action in actions.collection().iter().copied() {
        if world
            .entity(action)
            .get::<BEIBuffer<BEIContext>>()
            .is_some_and(buffer_contains_fired_input)
        {
            return action;
        }
    }
    let buffers: Vec<_> = actions
        .collection()
        .iter()
        .copied()
        .map(|action| {
            format!(
                "{action:?}: {:?}",
                world.entity(action).get::<BEIBuffer<BEIContext>>()
            )
        })
        .collect();
    panic!("{label} should have an action with fired rebroadcast input, got {buffers:?}");
}
