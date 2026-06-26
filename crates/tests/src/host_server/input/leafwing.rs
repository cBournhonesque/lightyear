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
use lightyear_sync::prelude::InputTimelineConfig;
use lightyear_sync::prelude::client::InputDelayConfig;
use test_log::test;
use tracing::info;

/// Check that ActionStates are stored correctly in the InputBuffer
/// when the host-client has an input delay configured
#[test]
fn test_buffer_inputs_with_delay() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::host_server());

    stepper.host_client_mut().insert(
        InputTimelineConfig::default().with_input_delay(InputDelayConfig::fixed_input_delay(1)),
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
    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");
    stepper
        .server_app
        .world_mut()
        .entity_mut(server_entity)
        .insert(InputMap::<LeafwingInput1>::new([(
            LeafwingInput1::Jump,
            KeyCode::KeyA,
        )]));
    stepper.frame_step(1);

    // press on a key
    stepper
        .server_app
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);
    stepper.frame_step(1);
    let server_tick = stepper.server_tick();

    // check that the action state got buffered without any press (because the input is delayed)
    // (we cannot use JustPressed because we start by ticking the ActionState)
    // (i.e. the InputBuffer is empty for the current tick, and has the button press only with 1 tick of delay)

    // TODO: REMOVE DEBUG LOGS
    let buffer = stepper
        .server_app
        .world()
        .entity(server_entity)
        .get::<LeafwingBuffer<LeafwingInput1>>();
    info!("InputBuffer: {buffer:?}");

    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<LeafwingBuffer<LeafwingInput1>>()
            .unwrap()
            .get(server_tick)
            .unwrap()
            .get_pressed()
            .is_empty()
    );
    // if we check the next tick (delay of 1), we can see that the InputBuffer contains the ActionState with a press
    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<LeafwingBuffer<LeafwingInput1>>()
            .unwrap()
            .get(server_tick + 1)
            .unwrap()
            .just_pressed(&LeafwingInput1::Jump)
    );

    // outside of the FixedUpdate schedule, the fixed_update_state of ActionState should be the delayed action
    // (which we restored)
    //
    // It has been ticked by LWIM so now it's only pressed
    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<ActionState<LeafwingInput1>>()
            .unwrap()
            .button_data(&LeafwingInput1::Jump)
            .unwrap()
            .fixed_update_state
            .pressed()
    );

    // release the key
    stepper
        .server_app
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::KeyA);

    // TODO: ideally we would check that the value of the ActionState inside FixedUpdate is correct
    // step another frame, this time we get the buffered input from earlier
    stepper.frame_step(1);
    let input_buffer = stepper
        .server_app
        .world()
        .entity(server_entity)
        .get::<LeafwingBuffer<LeafwingInput1>>()
        .unwrap();
    assert_eq!(
        input_buffer
            .get(server_tick + 1)
            .unwrap()
            .get_just_pressed(),
        &[LeafwingInput1::Jump]
    );
    // the fixed_update_state ActionState outside of FixedUpdate is the delayed one
    // it is `released` and not `just_released` because it was pressed in the previous frame
    assert!(
        stepper
            .server_app
            .world()
            .entity(server_entity)
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
            .server_app
            .world()
            .entity(server_entity)
            .get::<LeafwingBuffer<LeafwingInput1>>()
            .unwrap()
            .get(server_tick + 2)
            .unwrap()
            .just_released(&LeafwingInput1::Jump)
    );
}

/// `authorize_controlled_targets` exempts host-client inputs (`RemoteId::is_local`):
/// they are trusted in-process and must not be filtered against `ControlledByRemote`.
/// The host-client drives an entity with **no** `ControlledBy`; an observer chained
/// after the helper must still see the host-client's input target (the helper
/// skipped it). Were the `is_local` guard dropped, the target would be stripped
/// (the host-client controls nothing) and the observer would see nothing.
#[test]
fn test_authorize_controlled_targets_exempts_host_client() {
    use bevy::ecs::resource::Resource;
    use bevy::ecs::schedule::IntoScheduleConfigs;
    use bevy::ecs::system::{Query, ResMut};
    use bevy::prelude::Entity;
    use lightyear::input::leafwing::input_message::LeafwingSequence;
    use lightyear_inputs::input_message::{InputMessage, InputTarget};
    use lightyear_inputs::prelude::server::{InputSystems, authorize_controlled_targets};
    use lightyear_messages::prelude::MessageReceiver;

    #[derive(Resource, Default)]
    struct ObservedTargets(Vec<Entity>);

    fn observe(
        mut observed: ResMut<ObservedTargets>,
        mut receivers: Query<&mut MessageReceiver<InputMessage<LeafwingSequence<LeafwingInput1>>>>,
    ) {
        for mut receiver in &mut receivers {
            receiver.retain_messages(|msg| {
                for data in &msg.inputs {
                    if let InputTarget::Entity(e) = data.target {
                        observed.0.push(e);
                    }
                }
                true
            });
        }
    }

    let mut stepper = ClientServerStepper::from_config(StepperConfig::host_server());
    stepper.server_app.init_resource::<ObservedTargets>();
    stepper.server_app.add_systems(
        bevy::app::PreUpdate,
        (
            authorize_controlled_targets::<LeafwingSequence<LeafwingInput1>>,
            observe,
        )
            .chain()
            .in_set(InputSystems::ValidateInputs),
    );

    // Host-client-controlled entity with NO `ControlledBy`.
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionState::<LeafwingInput1>::default(),
            Replicate::to_clients(NetworkTarget::All),
            InputMap::<LeafwingInput1>::new([(LeafwingInput1::Jump, KeyCode::KeyA)]),
        ))
        .id();
    stepper.frame_step(2);
    stepper
        .server_app
        .world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyA);
    stepper.frame_step(5);

    assert!(
        stepper
            .server_app
            .world()
            .resource::<ObservedTargets>()
            .0
            .contains(&server_entity),
        "the host-client's input target was stripped (or never reached validation); \
         the is_local exemption is broken",
    );
}
