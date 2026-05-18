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
    use lightyear_replication::prelude::ControlledBy;

    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let client_of_0 = stepper.client_of(0).id();

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: client_of_0,
                lifetime: Default::default(),
            },
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
    use lightyear_replication::prelude::{ControlledBy, PredictionTarget};

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));
    let client_of_0 = stepper.client_of(0).id();

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: client_of_0,
                lifetime: Default::default(),
            },
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
    use lightyear_replication::prelude::{ControlledBy, PredictionTarget};

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));
    let client_of_0 = stepper.client_of(0).id();

    // Create an entity replicated to both clients with prediction
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: client_of_0,
                lifetime: Default::default(),
            },
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

/// Spoofed-target attack: client 0 emits an `InputMessage` whose
/// `InputTarget::Entity(...)` is client 1's controlled entity. The
/// defense in `is_input_target_authorized` should drop it.
///
/// Two assertions in one run:
/// - **Defense:** victim's server-side `Jump` MUST NOT be pressed.
/// - **Non-overblocking:** attacker's own server-side `Jump` MUST be
///   pressed (a defense that drops all cross-client inputs would fail
///   here).
#[test]
fn test_input_message_with_spoofed_target_is_rejected() {
    use lightyear_replication::prelude::ControlledBy;

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));

    let client_of_0 = stepper.client_of(0).id();
    let client_of_1 = stepper.client_of(1).id();

    let victim_server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: client_of_1,
                lifetime: Default::default(),
            },
        ))
        .id();

    let attacker_server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: client_of_0,
                lifetime: Default::default(),
            },
        ))
        .id();

    // Warm-up: `ControlledByRemote` must auto-populate on the peer
    // connection entity before the receive-path filter accepts inputs.
    stepper.frame_step(10);

    let victim_local_on_client_0 = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(victim_server_entity)
        .expect("victim entity should be replicated to client 0");
    let attacker_local_on_client_0 = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(attacker_server_entity)
        .expect("attacker entity should be replicated to client 0");

    // KeyJ on victim's replica = the attack; KeyA on attacker's own = the
    // legitimate positive control. `InputMap` requires `InputBuffer`, no
    // explicit buffer insertion needed.
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(victim_local_on_client_0)
        .insert(InputMap::<LeafwingInput1>::new([(
            LeafwingInput1::Jump,
            KeyCode::KeyJ,
        )]));
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(attacker_local_on_client_0)
        .insert(InputMap::<LeafwingInput1>::new([(
            LeafwingInput1::Jump,
            KeyCode::KeyA,
        )]));

    // leafwing needs a frame to register the InputMap before key presses
    // produce action-state transitions.
    stepper.frame_step(1);

    stepper.client_apps[0]
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyJ);
    stepper.client_apps[0]
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);

    stepper.frame_step(20);

    let victim_server_state = stepper
        .server_app
        .world()
        .entity(victim_server_entity)
        .get::<ActionState<LeafwingInput1>>()
        .expect("victim has ActionState");
    assert!(
        !victim_server_state.pressed(&LeafwingInput1::Jump),
        "Server applied client 0's forged input to client 1's entity \
         (`ActionState::pressed` shows Jump pressed). The input-message \
         receive path does not authorize the target entity against \
         the sender's ControlledByRemote — see \
         `is_input_target_authorized` in lightyear_inputs::server.",
    );

    let attacker_server_state = stepper
        .server_app
        .world()
        .entity(attacker_server_entity)
        .get::<ActionState<LeafwingInput1>>()
        .expect("attacker has ActionState");
    assert!(
        attacker_server_state.pressed(&LeafwingInput1::Jump),
        "Client 0's legitimate Jump input for its OWN entity did not \
         reach the server (`ActionState::pressed` shows Jump released). \
         The authorization defense is dropping authorized inputs too \
         — broken in the opposite direction from the spoofed-target \
         case.",
    );
}

/// Exercises the empty-after-filter early-return branch in
/// `receive_input_message`: client 0 controls nothing, forges an input
/// targeting client 1's entity, retain drops it, `inputs.is_empty()`
/// fires before the rebroadcast / per-target-apply blocks.
#[test]
fn test_input_message_with_only_spoofed_targets_filters_to_empty() {
    use lightyear_replication::prelude::ControlledBy;

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));
    let client_of_1 = stepper.client_of(1).id();

    let victim_server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: client_of_1,
                lifetime: Default::default(),
            },
        ))
        .id();

    stepper.frame_step(3);

    let victim_local_on_client_0 = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(victim_server_entity)
        .expect("victim entity should be replicated to client 0");

    stepper.client_apps[0]
        .world_mut()
        .entity_mut(victim_local_on_client_0)
        .insert(InputMap::<LeafwingInput1>::new([(
            LeafwingInput1::Jump,
            KeyCode::KeyJ,
        )]));
    stepper.frame_step(1);
    stepper.client_apps[0]
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyJ);

    stepper.frame_step(10);

    let victim_server_state = stepper
        .server_app
        .world()
        .entity(victim_server_entity)
        .get::<ActionState<LeafwingInput1>>()
        .expect("victim has ActionState");
    assert!(
        !victim_server_state.pressed(&LeafwingInput1::Jump),
        "spoofed-target input was applied despite client 0 controlling \
         nothing — the empty-after-filter early-return path is broken \
         or the upstream auth filter is missing.",
    );
}

/// End_tick DOS: a forged `InputMessage` with `end_tick = server_tick +
/// 30_000` causes `InputBuffer::set_raw` to allocate ~30k entries.
/// Goes end-to-end via the public `MessageSender::send::<InputChannel>`
/// API (no internal-API hooks) and asserts the server's buffer stays
/// bounded. See `is_input_within_lookahead` for the defense.
#[test]
fn test_input_message_with_huge_end_tick_does_not_allocate_unbounded_buffer() {
    use lightyear::input::leafwing::input_message::{LeafwingSequence, LeafwingSnapshot};
    use lightyear_inputs::input_buffer::InputBuffer;
    use lightyear_inputs::input_message::{
        ActionStateSequence, InputMessage, InputTarget, PerTargetData,
    };
    use lightyear_inputs::prelude::InputChannel;
    use lightyear_messages::prelude::MessageSender;
    use lightyear_replication::prelude::ControlledBy;

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(1));
    let client_of_0 = stepper.client_of(0).id();

    let target_server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
            ControlledBy {
                owner: client_of_0,
                lifetime: Default::default(),
            },
        ))
        .id();

    // Warm-up: `ControlledByRemote` must auto-populate before the
    // receive-path filter accepts inputs.
    stepper.frame_step(5);

    let target_local = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(target_server_entity)
        .expect("target entity should be replicated to client 0");

    // Establish a baseline server-side InputBuffer at the current tick
    // range. Without this the attack just initializes the buffer at the
    // huge tick (no gap to fill, no growth).
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(target_local)
        .insert(InputMap::<LeafwingInput1>::new([(
            LeafwingInput1::Jump,
            KeyCode::KeyA,
        )]));
    stepper.frame_step(1);
    stepper.client_apps[0]
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);
    stepper.frame_step(5);

    let server_tick_before = stepper.server_tick();
    let attack_end_tick = server_tick_before + 30_000;

    // The sequence content doesn't matter — only `end_tick` controls
    // how far `set_raw` extends the server's buffer.
    let mut sequence_buf =
        InputBuffer::<LeafwingSnapshot<LeafwingInput1>, LeafwingInput1>::default();
    let mut snapshot_state = ActionState::<LeafwingInput1>::default();
    snapshot_state.press(&LeafwingInput1::Jump);
    sequence_buf.set(attack_end_tick, LeafwingSnapshot(snapshot_state));
    let sequence = LeafwingSequence::<LeafwingInput1>::build_from_input_buffer(
        &sequence_buf,
        1,
        attack_end_tick,
    )
    .expect("sequence built from non-empty buffer");

    let mut forged: InputMessage<LeafwingSequence<LeafwingInput1>> =
        InputMessage::new(attack_end_tick);
    forged.inputs.push(PerTargetData {
        target: InputTarget::Entity(target_local),
        states: sequence,
    });

    // Send via the public MessageSender API — models exactly what a
    // modified client binary could do.
    {
        let client_app = &mut stepper.client_apps[0];
        let mut client_entity_mut = client_app
            .world_mut()
            .entity_mut(stepper.client_entities[0]);
        let mut sender = client_entity_mut
            .get_mut::<MessageSender<InputMessage<LeafwingSequence<LeafwingInput1>>>>()
            .expect("client has a MessageSender for InputMessage<LeafwingSequence>");
        sender.send::<InputChannel>(forged);
    }

    // Frames for serialize → wire → deserialize → receive.
    stepper.frame_step(3);

    let buffer = stepper
        .server_app
        .world()
        .entity(target_server_entity)
        .get::<InputBuffer<LeafwingSnapshot<LeafwingInput1>, LeafwingInput1>>()
        .expect("server should have an InputBuffer for the target after receiving inputs");
    let buffer_len = buffer.len();
    assert!(
        buffer_len < 1_000,
        "DOS: server allocated {buffer_len} entries in the InputBuffer for a \
         single forged InputMessage with end_tick = server_tick + 30000. The \
         receive path does not bound the message's end_tick — see \
         `is_input_within_lookahead` in lightyear_inputs::server.",
    );
}
