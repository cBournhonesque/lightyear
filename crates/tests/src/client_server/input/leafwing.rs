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

/// End_tick DoS: a forged `InputMessage` with `end_tick = server_tick +
/// 30_000` causes `InputBuffer::set_raw` to allocate ~30k entries. Goes
/// end-to-end via the public `MessageSender::send::<InputChannel>` API (no
/// internal-API hooks) and asserts the server's buffer stays bounded. See
/// `is_input_within_lookahead` in `lightyear_inputs::server` for the defense.
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

    // Warm-up: let the entity replicate down to the client and (if the
    // target-authorization defense is also present) `ControlledByRemote`
    // auto-populate, so the forged input is authorized and actually reaches
    // `set_raw` — exercising the DoS path rather than being filtered first.
    stepper.frame_step(5);

    let target_local = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(target_server_entity)
        .expect("target entity should be replicated to client 0");

    // Establish a baseline server-side InputBuffer at the current tick range.
    // Without this the attack just initializes the buffer at the huge tick
    // (no gap to fill, no growth).
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

    // The sequence content doesn't matter — only `end_tick` controls how far
    // `set_raw` extends the server's buffer.
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

    // Send via the public MessageSender API — models exactly what a modified
    // client binary could do.
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
        "DoS: server allocated {buffer_len} entries in the InputBuffer for a \
         single forged InputMessage with end_tick = server_tick + 30000. The \
         receive path does not bound the message's end_tick — see \
         `is_input_within_lookahead` in lightyear_inputs::server.",
    );
}

/// Example + test for the game-side input-validation seam: a normal Bevy system
/// registered with `add_input_validator` runs in `InputSystems::ValidateInputs`
/// (after receive, before buffering) with **full ECS access**, and mutates/drops
/// received `InputMessage`s in place via `MessageReceiver::retain_messages`.
///
/// Here the validator reads a `Res<RejectInputs>` (proving arbitrary `SystemParam`
/// access) and drops every input message while the flag is set, so a legitimate,
/// authorized key press never reaches the server's `ActionState`.
#[test]
fn test_input_validator_system_can_drop_messages() {
    use bevy::ecs::resource::Resource;
    use bevy::ecs::system::{Query, Res};
    use lightyear::input::leafwing::input_message::LeafwingSequence;
    use lightyear_inputs::input_message::InputMessage;
    use lightyear_inputs::prelude::server::InputValidationAppExt;
    use lightyear_messages::prelude::MessageReceiver;

    #[derive(Resource)]
    struct RejectInputs(bool);

    // A game-side validation system: full ECS access (reads a resource), drops
    // the input messages in place. A real validator would clamp/inspect against
    // game state rather than reject wholesale.
    fn reject_inputs(
        reject: Res<RejectInputs>,
        mut receivers: Query<&mut MessageReceiver<InputMessage<LeafwingSequence<LeafwingInput1>>>>,
    ) {
        if !reject.0 {
            return;
        }
        for mut receiver in &mut receivers {
            receiver.retain_messages(|_msg| false);
        }
    }

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(1));
    stepper.server_app.insert_resource(RejectInputs(true));
    stepper.server_app.add_input_validator(reject_inputs);

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
        "input reached the server even though the validation system dropped \
         every message in ValidateInputs — the seam isn't running before \
         ReceiveInputs, or retain_messages didn't take effect.",
    );
}

/// Example: a *game-supplied* `ValidateInputs` system implements input-target
/// authorization itself — lightyear does not enforce `ControlledBy` (it's an
/// optional helper). The validator drops any `InputTarget::Entity` the sender
/// doesn't control (via `ControlledByRemote` + `retain_messages`).
///
/// Client 0 controls entity A and forges an input also targeting entity B
/// (uncontrolled). The validator must let A's input through (non-overblocking)
/// and drop B's — so A's server `ActionState` is pressed and B's is not.
#[test]
fn test_user_validator_can_authorize_targets() {
    use bevy::ecs::relationship::RelationshipTarget;
    use bevy::ecs::system::Query;
    use lightyear::input::leafwing::input_message::LeafwingSequence;
    use lightyear_core::id::RemoteId;
    use lightyear_inputs::input_message::{InputMessage, InputTarget};
    use lightyear_inputs::prelude::server::InputValidationAppExt;
    use lightyear_messages::prelude::MessageReceiver;
    use lightyear_replication::control::ControlledByRemote;
    use lightyear_replication::prelude::ControlledBy;

    // Game-side authorization, expressed as an ordinary ValidateInputs system.
    fn authorize_targets(
        mut receivers: Query<(
            &RemoteId,
            Option<&ControlledByRemote>,
            &mut MessageReceiver<InputMessage<LeafwingSequence<LeafwingInput1>>>,
        )>,
    ) {
        for (client_id, controlled, mut receiver) in &mut receivers {
            if client_id.is_local() {
                continue;
            }
            receiver.retain_messages(|msg| {
                msg.inputs.retain(|data| match data.target {
                    InputTarget::Entity(e) => {
                        controlled.is_some_and(|c| c.collection().contains(&e))
                    }
                    InputTarget::PreSpawned(_) => true,
                });
                !msg.inputs.is_empty()
            });
        }
    }

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(1));
    stepper.server_app.add_input_validator(authorize_targets);

    let client_of_0 = stepper.client_of(0).id();
    // Entity A: controlled by client 0.
    let entity_a = stepper
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
    // Entity B: replicated to client 0 but NOT controlled by it (the spoof victim).
    let entity_b = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    stepper.frame_step(10);

    let local_a = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(entity_a)
        .expect("A replicated to client 0");
    let local_b = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(entity_b)
        .expect("B replicated to client 0");

    // Client 0 puts an InputMap on BOTH its own entity and the victim's, so its
    // outgoing message targets A (legit) and B (spoofed).
    for local in [local_a, local_b] {
        stepper.client_apps[0].world_mut().entity_mut(local).insert(
            InputMap::<LeafwingInput1>::new([(LeafwingInput1::Jump, KeyCode::KeyA)]),
        );
    }
    stepper.frame_step(1);
    stepper.client_apps[0]
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);
    stepper.frame_step(10);

    // Non-overblocking: A's authorized input reached the server.
    assert!(
        stepper
            .server_app
            .world()
            .entity(entity_a)
            .get::<ActionState<LeafwingInput1>>()
            .unwrap()
            .pressed(&LeafwingInput1::Jump),
        "the authorized input for A did not land — the validator over-stripped",
    );
    // The spoofed input for B was dropped by the validator.
    assert!(
        !stepper
            .server_app
            .world()
            .entity(entity_b)
            .get::<ActionState<LeafwingInput1>>()
            .unwrap()
            .pressed(&LeafwingInput1::Jump),
        "spoofed input landed on victim B's ActionState",
    );
}

/// `retain_received_messages` exposes per-message metadata (`remote_tick`,
/// `channel_kind`, `message_id`) that `retain_messages` hides — needed for
/// rate-limit / tick-window / replay validators. Here a validator reads
/// `remote_tick` and drops the message; we assert both that the metadata was
/// readable and that the drop took effect.
#[test]
fn test_validator_can_read_message_metadata() {
    use bevy::ecs::resource::Resource;
    use bevy::ecs::system::{Query, ResMut};
    use lightyear::input::leafwing::input_message::LeafwingSequence;
    use lightyear_inputs::input_message::InputMessage;
    use lightyear_inputs::prelude::server::InputValidationAppExt;
    use lightyear_messages::prelude::MessageReceiver;

    #[derive(Resource, Default)]
    struct SeenRemoteTick(Option<u32>);

    fn inspect_metadata(
        mut seen: ResMut<SeenRemoteTick>,
        mut receivers: Query<&mut MessageReceiver<InputMessage<LeafwingSequence<LeafwingInput1>>>>,
    ) {
        for mut receiver in &mut receivers {
            receiver.retain_received_messages(|metadata, _data| {
                // Metadata is reachable here (read-only), unlike `retain_messages`.
                seen.0 = Some(metadata.remote_tick.0);
                false // drop the message
            });
        }
    }

    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(1));
    stepper.server_app.init_resource::<SeenRemoteTick>();
    stepper.server_app.add_input_validator(inspect_metadata);

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
        stepper
            .server_app
            .world()
            .resource::<SeenRemoteTick>()
            .0
            .is_some(),
        "validator never observed a message's remote_tick metadata",
    );
    assert!(
        !stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<ActionState<LeafwingInput1>>()
            .unwrap()
            .pressed(&LeafwingInput1::Jump),
        "input landed even though retain_received_messages dropped the message",
    );
}
