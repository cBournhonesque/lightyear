use crate::protocol::LeafwingInput1;
use crate::stepper::ClientServerStepper;
use bevy::input::ButtonInput;
use bevy::prelude::KeyCode;
use leafwing_input_manager::action_state::ActionState;
use leafwing_input_manager::prelude::InputMap;
use lightyear::input::input_buffer::InputBuffer;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::prelude::Timeline;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::Replicate;
use lightyear_sync::prelude::client::{Input, InputDelayConfig};
use lightyear_sync::prelude::InputTimeline;
use test_log::test;

/// Check that ActionStates are stored correctly in the InputBuffer
#[test]
fn test_buffer_inputs_with_delay() {
    let mut stepper = ClientServerStepper::single();
    stepper.client_mut(0).insert(
        InputTimeline(Timeline::from(
            Input::default().with_input_delay(InputDelayConfig::fixed_input_delay(1)),
        )),
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
    stepper.frame_step(1);
    let client_tick = stepper.client_tick(0);

    // check that the action state got buffered without any press (because the input is delayed)
    // (we cannot use JustPressed because we start by ticking the ActionState)
    // (i.e. the InputBuffer is empty for the current tick, and has the button press only with 1 tick of delay)
    assert!(stepper
        .client_app()
        .world()
        .entity(client_entity)
        .get::<InputBuffer<ActionState<LeafwingInput1>>>()
        .unwrap()
        .get(client_tick)
        .unwrap()
        .get_pressed()
        .is_empty());
    // if we check the next tick (delay of 1), we can see that the InputBuffer contains the ActionState with a press
    assert!(stepper
        .client_app()
        .world()
        .entity(client_entity)
        .get::<InputBuffer<ActionState<LeafwingInput1>>>()
        .unwrap()
        .get(client_tick + 1)
        .unwrap()
        .just_pressed(&LeafwingInput1::Jump));

    // outside of the FixedUpdate schedule, the fixed_update_state of ActionState should be the delayed action
    // (which we restored)
    //
    // It has been ticked by LWIM so now it's only pressed
    assert!(stepper
        .client_app()
        .world()
        .entity(client_entity)
        .get::<ActionState<LeafwingInput1>>()
        .unwrap()
        .button_data(&LeafwingInput1::Jump)
        .unwrap()
        .fixed_update_state
        .pressed());

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
        .get::<InputBuffer<ActionState<LeafwingInput1>>>()
        .unwrap();
    assert_eq!(
        input_buffer.get(client_tick + 1).unwrap().get_just_pressed(),
        &[LeafwingInput1::Jump]
    );
    // the fixed_update_state ActionState outside of FixedUpdate is the delayed one
    assert!(stepper
        .client_app()
        .world()
        .entity(client_entity)
        .get::<ActionState<LeafwingInput1>>()
        .unwrap()
        .button_data(&LeafwingInput1::Jump)
        .unwrap()
        .fixed_update_state
        .just_released());

    stepper.frame_step(1);

    assert!(stepper
        .client_app()
        .world()
        .entity(client_entity)
        .get::<InputBuffer<ActionState<LeafwingInput1>>>()
        .unwrap()
        .get(client_tick + 2)
        .unwrap()
        .just_released(&LeafwingInput1::Jump));
}