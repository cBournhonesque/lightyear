use crate::protocol::LeafwingInput1;
use crate::stepper::*;
use bevy::input::ButtonInput;
use bevy::prelude::KeyCode;
use leafwing_input_manager::action_state::ActionState;
use leafwing_input_manager::prelude::InputMap;
use lightyear::input::leafwing::prelude::LeafwingBuffer;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::Replicate;
use lightyear_sync::prelude::client::InputDelayConfig;
use lightyear_sync::prelude::*;
use test_log::test;

/// Check that ActionStates are stored correctly in the InputBuffer
#[test]
fn test_buffer_inputs_with_delay() {
    let mut config = StepperConfig::single();
    config.init = false;
    let mut stepper = ClientServerStepper::from_config(config);
    stepper.client_mut(0).insert(
        InputTimelineConfig::default().with_input_delay(InputDelayConfig::fixed_input_delay(1)),
    );
    stepper.init();
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(2);
    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(InputMap::<LeafwingInput1>::new([(
            LeafwingInput1::Jump,
            KeyCode::KeyA,
        )]));
    stepper.frame_step(1);

    // press on a key
    stepper
        .client_app()
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);
    tracing::info!("tick: {:?}", stepper.client_tick(0));
    stepper.frame_step(1);
    let client_tick = stepper.client_tick(0);
    let buffer = stepper
        .client_app()
        .world()
        .entity(client_entity)
        .get::<LeafwingBuffer<LeafwingInput1>>()
        .unwrap();
    tracing::info!(?client_tick, ?buffer);

    // check that the action state got buffered without any press (because the input is delayed)
    // (we cannot use JustPressed because we start by ticking the ActionState)
    // (i.e. the InputBuffer is empty for the current tick, and has the button press only with 1 tick of delay)
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .get::<LeafwingBuffer<LeafwingInput1>>()
            .unwrap()
            .get(client_tick)
            .unwrap()
            .get_pressed()
            .is_empty()
    );
    // if we check the next tick (delay of 1), we can see that the InputBuffer contains the ActionState with a press
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .get::<LeafwingBuffer<LeafwingInput1>>()
            .unwrap()
            .get(client_tick + 1)
            .unwrap()
            .just_pressed(&LeafwingInput1::Jump)
    );

    // outside of the FixedUpdate schedule, the fixed_update_state of ActionState should be the delayed action
    // (which we restored)
    //
    // It has been ticked by LWIM so now it's only pressed
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .get::<ActionState<LeafwingInput1>>()
            .unwrap()
            .button_data(&LeafwingInput1::Jump)
            .unwrap()
            .fixed_update_state
            .pressed()
    );

    // release the key
    stepper
        .client_app()
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::KeyA);

    // TODO: ideally we would check that the value of the ActionState inside FixedUpdate is correct
    // step another frame, this time we get the buffered input from earlier
    stepper.frame_step(1);
    let input_buffer = stepper
        .client_app()
        .world()
        .entity(client_entity)
        .get::<LeafwingBuffer<LeafwingInput1>>()
        .unwrap();
    assert_eq!(
        input_buffer
            .get(client_tick + 1)
            .unwrap()
            .get_just_pressed(),
        &[LeafwingInput1::Jump]
    );
    // the fixed_update_state ActionState outside of FixedUpdate is the delayed one
    // It has been ticked by LWIM so now it's only released and not just_released
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .get::<ActionState<LeafwingInput1>>()
            .unwrap()
            .button_data(&LeafwingInput1::Jump)
            .unwrap()
            .fixed_update_state
            .released()
    );

    stepper.frame_step(1);

    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .get::<LeafwingBuffer<LeafwingInput1>>()
            .unwrap()
            .get(client_tick + 2)
            .unwrap()
            .just_released(&LeafwingInput1::Jump)
    );
}

/// Verify that `from_snapshot_transitions` produces correct `just_pressed` /
/// `just_released` transitions when applying a network snapshot.
///
/// The wire format (`ActionDiff`) collapses `JustPressed` into `Pressed`.
/// `from_snapshot` (raw clone) would lose the transition. `from_snapshot_transitions`
/// instead compares the current state against the snapshot and calls
/// `press()` / `release()` to produce the correct transition.
#[test]
fn test_from_snapshot_transitions_produces_just_pressed() {
    use lightyear::input::input_message::ActionStateSequence;
    use lightyear::input::leafwing::input_message::{LeafwingSequence, LeafwingSnapshot};

    let now = std::time::Instant::now();

    // Start with a released state.
    let mut state = ActionState::<LeafwingInput1>::default();

    // Create a snapshot where the button is pressed (simulating what arrives
    // over the wire — `Pressed`, not `JustPressed`).
    let mut pressed_snapshot = ActionState::<LeafwingInput1>::default();
    pressed_snapshot.press(&LeafwingInput1::Jump);
    // Tick to advance JustPressed → Pressed (simulating the wire collapse).
    pressed_snapshot.tick(now, now);
    assert!(
        pressed_snapshot.pressed(&LeafwingInput1::Jump),
        "snapshot should be Pressed"
    );
    assert!(
        !pressed_snapshot.just_pressed(&LeafwingInput1::Jump),
        "snapshot should NOT be JustPressed (collapsed by wire format)"
    );

    let snapshot = LeafwingSnapshot(pressed_snapshot);

    // Apply with from_snapshot_transitions: should detect the transition
    // from Released → Pressed and produce JustPressed.
    LeafwingSequence::<LeafwingInput1>::from_snapshot_transitions(&mut state, &snapshot);
    assert!(
        state.just_pressed(&LeafwingInput1::Jump),
        "from_snapshot_transitions should produce JustPressed"
    );

    // Apply again — now it's Pressed → Pressed, so just_pressed should be false.
    LeafwingSequence::<LeafwingInput1>::from_snapshot_transitions(&mut state, &snapshot);
    assert!(
        state.pressed(&LeafwingInput1::Jump),
        "should still be pressed"
    );
    assert!(
        !state.just_pressed(&LeafwingInput1::Jump),
        "should NOT be just_pressed on second application"
    );

    // Now apply a released snapshot — should produce just_released.
    let released_snapshot = LeafwingSnapshot(ActionState::<LeafwingInput1>::default());
    LeafwingSequence::<LeafwingInput1>::from_snapshot_transitions(&mut state, &released_snapshot);
    assert!(
        state.just_released(&LeafwingInput1::Jump),
        "from_snapshot_transitions should produce JustReleased"
    );
}

/// Verify that `just_pressed` works correctly on the server after receiving
/// a client's input.
///
/// The wire format (`ActionDiff`) collapses `JustPressed` into `Pressed`,
/// so the server must reconstruct the transition from the state change.
/// Currently this test documents the known limitation: `just_pressed()`
/// returns false on the server because `from_snapshot` raw-clones the
/// `ActionState` without detecting transitions.
#[test]
fn test_server_just_pressed() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(2);

    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity not replicated to client");

    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(InputMap::<LeafwingInput1>::new([(
            LeafwingInput1::Jump,
            KeyCode::KeyA,
        )]));
    stepper.frame_step(1);

    stepper
        .client_app()
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);

    stepper.frame_step(3);

    let server_action = stepper
        .server_app
        .world()
        .entity(server_entity)
        .get::<ActionState<LeafwingInput1>>()
        .unwrap();
    assert!(
        server_action.pressed(&LeafwingInput1::Jump),
        "Server should see the button as pressed"
    );

    // Document current behavior until the server-side transition reconstruction
    // path is fully enabled.
    assert!(
        !server_action.just_pressed(&LeafwingInput1::Jump),
        "KNOWN BUG: just_pressed is lost on the server (see PR #1438)"
    );
}

/// Test that leafwing inputs from one client are rebroadcasted to other clients.
///
/// Client 0 presses a button. The server receives it, rebroadcasts to client 1.
/// Client 1 should see the pressed state in its InputBuffer for the remote player.
#[test]
fn test_leafwing_input_rebroadcast() {
    use lightyear_replication::prelude::PredictionTarget;

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    // Create an entity replicated to both clients with prediction
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step_server_first(1);

    let client0_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity not replicated to client 0");

    let client1_entity = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity not replicated to client 1");

    // Client 0 has an InputMap so it can drive inputs
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(client0_entity)
        .insert(InputMap::<LeafwingInput1>::new([(
            LeafwingInput1::Jump,
            KeyCode::KeyA,
        )]));

    stepper.frame_step(1);

    // Press button on client 0
    stepper.client_apps[0]
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);

    // Let it propagate: client 0 → server → rebroadcast to client 1
    stepper.frame_step(5);

    // Check that client 1 received the rebroadcasted input
    let client1_has_buffer = stepper.client_apps[1]
        .world()
        .entity(client1_entity)
        .get::<LeafwingBuffer<LeafwingInput1>>()
        .is_some();

    assert!(
        client1_has_buffer,
        "Client 1 should have an InputBuffer for the remote player after receiving rebroadcasted inputs"
    );

    if client1_has_buffer {
        let buffer = stepper.client_apps[1]
            .world()
            .entity(client1_entity)
            .get::<LeafwingBuffer<LeafwingInput1>>()
            .unwrap();
        assert!(
            buffer.last_remote_tick.is_some(),
            "Client 1's InputBuffer should have a last_remote_tick from the rebroadcast"
        );
    }
}
