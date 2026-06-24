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

    // The input timeline sends inputs one tick ahead of the server input pipeline.
    // Step until the server applies the first pressed snapshot, not just until it receives it.
    stepper.frame_step(4);

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

    assert!(
        server_action.just_pressed(&LeafwingInput1::Jump),
        "Server should reconstruct JustPressed from the received input transition"
    );
}

/// When a rebroadcast creates a new InputBuffer on an entity that didn't have
/// one yet, the ActionState should be initialized from the latest buffered input,
/// not from base_value(). Otherwise the remote player simulates with empty/released
/// inputs until the buffer catches up to the current tick.
#[test]
fn test_rebroadcast_initializes_action_state_from_buffer() {
    use lightyear_replication::prelude::PredictionTarget;

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

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

    // Client 0 drives inputs
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

    // Let it propagate
    stepper.frame_step(5);

    // Client 1's ActionState for the remote player should reflect the pressed
    // button, not be empty
    let action_state = stepper.client_apps[1]
        .world()
        .entity(client1_entity)
        .get::<ActionState<LeafwingInput1>>()
        .expect("Client 1 should have ActionState for remote player");

    assert!(
        action_state.pressed(&LeafwingInput1::Jump),
        "ActionState on client 1 should have Jump pressed after rebroadcast, got: {:?}",
        action_state.get_pressed()
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

// --- Input-message validation framework --------------------------------------
//
// These exercise the pluggable validator chain (lightyear ships no validators
// by default, so without registration the chain is a no-op). They demonstrate:
// rejecting a whole message, mutating a message (dropping targets), and
// removing a registered validator by name.

/// A registered validator that returns `Reject` drops the message: a
/// legitimate, authorized input never reaches the server's `ActionState`.
/// Registered here as a plain closure (blanket `Fn` impl).
#[test]
fn test_registered_validator_can_reject_input() {
    use lightyear::input::leafwing::input_message::LeafwingSequence;
    use lightyear_inputs::input_message::InputMessage;
    use lightyear_inputs::prelude::server::{
        InputValidation, InputValidationContext, InputValidatorAppExt,
    };

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(1));
    stepper
        .server_app
        .add_input_validator::<LeafwingSequence<LeafwingInput1>>(
            |_ctx: &InputValidationContext<LeafwingSequence<LeafwingInput1>>,
             _msg: &mut InputMessage<LeafwingSequence<LeafwingInput1>>| {
                InputValidation::Reject
            },
        );

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(2);

    let local = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity replicated to client 0");
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(local)
        .insert(InputMap::<LeafwingInput1>::new([(
            LeafwingInput1::Jump,
            KeyCode::KeyA,
        )]));
    stepper.frame_step(1);
    stepper.client_apps[0]
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);
    stepper.frame_step(10);

    let server_state = stepper
        .server_app
        .world()
        .entity(server_entity)
        .get::<ActionState<LeafwingInput1>>()
        .expect("entity has ActionState");
    assert!(
        !server_state.pressed(&LeafwingInput1::Jump),
        "input reached the server even though a registered validator rejected \
         every message — the validator chain is not being run.",
    );
}

/// A validator may mutate the message instead of rejecting it. Here it drops
/// the message's `InputTarget::Entity` entries via `retain` (returning
/// `Continue`), so the authorized input still never lands — exercising the
/// mutate path and the empty-after-mutation no-op.
#[test]
fn test_validator_can_drop_targets_by_mutation() {
    use lightyear::input::leafwing::input_message::LeafwingSequence;
    use lightyear_inputs::input_message::InputMessage;
    use lightyear_inputs::prelude::server::{
        InputValidation, InputValidationContext, InputValidatorAppExt,
    };

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(1));
    stepper
        .server_app
        .add_input_validator::<LeafwingSequence<LeafwingInput1>>(
            |_ctx: &InputValidationContext<LeafwingSequence<LeafwingInput1>>,
             msg: &mut InputMessage<LeafwingSequence<LeafwingInput1>>| {
                // Drop every entity target; keep the message otherwise intact.
                msg.inputs.clear();
                InputValidation::Continue
            },
        );

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(2);

    let local = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity replicated to client 0");
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(local)
        .insert(InputMap::<LeafwingInput1>::new([(
            LeafwingInput1::Jump,
            KeyCode::KeyA,
        )]));
    stepper.frame_step(1);
    stepper.client_apps[0]
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);
    stepper.frame_step(10);

    let server_state = stepper
        .server_app
        .world()
        .entity(server_entity)
        .get::<ActionState<LeafwingInput1>>()
        .expect("entity has ActionState");
    assert!(
        !server_state.pressed(&LeafwingInput1::Jump),
        "input landed even though the validator cleared the message's targets.",
    );
}

/// `remove_input_validator(name)` takes a previously-registered validator back
/// out of the chain: the same input that was blocked now flows through.
#[test]
fn test_remove_input_validator_restores_handling() {
    use lightyear::input::leafwing::input_message::LeafwingSequence;
    use lightyear_inputs::input_message::InputMessage;
    use lightyear_inputs::prelude::server::{
        InputMessageValidator, InputValidation, InputValidationContext, InputValidatorAppExt,
    };

    /// A named validator that rejects everything.
    struct BlockAll;
    impl BlockAll {
        const NAME: &'static str = "test::BlockAll";
    }
    impl InputMessageValidator<LeafwingSequence<LeafwingInput1>> for BlockAll {
        fn validate(
            &self,
            _ctx: &InputValidationContext<'_, LeafwingSequence<LeafwingInput1>>,
            _msg: &mut InputMessage<LeafwingSequence<LeafwingInput1>>,
        ) -> InputValidation {
            InputValidation::Reject
        }
        fn name(&self) -> &'static str {
            Self::NAME
        }
    }

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(1));
    stepper
        .server_app
        .add_input_validator::<LeafwingSequence<LeafwingInput1>>(BlockAll);

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(2);

    let local = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity replicated to client 0");
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(local)
        .insert(InputMap::<LeafwingInput1>::new([(
            LeafwingInput1::Jump,
            KeyCode::KeyA,
        )]));
    stepper.frame_step(1);
    stepper.client_apps[0]
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);
    stepper.frame_step(10);

    let blocked = stepper
        .server_app
        .world()
        .entity(server_entity)
        .get::<ActionState<LeafwingInput1>>()
        .unwrap()
        .pressed(&LeafwingInput1::Jump);
    assert!(!blocked, "BlockAll should have rejected the input");

    // Remove the validator; the same held key should now reach the server.
    stepper
        .server_app
        .remove_input_validator::<LeafwingSequence<LeafwingInput1>>(BlockAll::NAME);
    stepper.frame_step(10);

    let now_pressed = stepper
        .server_app
        .world()
        .entity(server_entity)
        .get::<ActionState<LeafwingInput1>>()
        .unwrap()
        .pressed(&LeafwingInput1::Jump);
    assert!(
        now_pressed,
        "after remove_input_validator, the input should reach the server, but \
         it is still being dropped.",
    );
}

/// A validator can read a target's current server-side `InputBuffer` through
/// the context accessor. The validator records (via a captured flag) whether it
/// ever resolved a buffer, and returns `Continue` so the input still lands —
/// proving the accessor resolves real targets without altering behavior.
#[test]
fn test_validator_can_read_target_input_buffer() {
    use lightyear::input::leafwing::input_message::LeafwingSequence;
    use lightyear_inputs::input_message::InputMessage;
    use lightyear_inputs::prelude::server::{
        InputValidation, InputValidationContext, InputValidatorAppExt,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let saw_buffer = Arc::new(AtomicBool::new(false));
    let flag = saw_buffer.clone();

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(1));
    stepper
        .server_app
        .add_input_validator::<LeafwingSequence<LeafwingInput1>>(
            move |ctx: &InputValidationContext<LeafwingSequence<LeafwingInput1>>,
                  msg: &mut InputMessage<LeafwingSequence<LeafwingInput1>>| {
                for data in &msg.inputs {
                    if ctx.input_buffer(data.target).is_some() {
                        flag.store(true, Ordering::SeqCst);
                    }
                }
                InputValidation::Continue
            },
        );

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(2);

    let local = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity replicated to client 0");
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(local)
        .insert(InputMap::<LeafwingInput1>::new([(
            LeafwingInput1::Jump,
            KeyCode::KeyA,
        )]));
    stepper.frame_step(1);
    stepper.client_apps[0]
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);
    stepper.frame_step(10);

    assert!(
        saw_buffer.load(Ordering::SeqCst),
        "validator never resolved a target's InputBuffer via ctx.input_buffer — \
         the read-only buffer accessor is not wired.",
    );
    let pressed = stepper
        .server_app
        .world()
        .entity(server_entity)
        .get::<ActionState<LeafwingInput1>>()
        .unwrap()
        .pressed(&LeafwingInput1::Jump);
    assert!(
        pressed,
        "validator returned Continue, so the legitimate input should still land.",
    );
}
