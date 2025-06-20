use crate::protocol::{BEIAction1, BEIContext};
use crate::stepper::ClientServerStepper;
use bevy::app::App;
use bevy::ecs::system::SystemState;
use bevy::prelude::Res;
use bevy_enhanced_input::prelude::{ActionState, Actions, MockSpan};
use lightyear::input::bei;
use lightyear::input::bei::input_message::BEIStateSequence;
use lightyear::input::input_buffer::InputBuffer;
use lightyear::input::input_message::ActionStateSequence;
use lightyear_core::prelude::{Tick, Timeline};
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::Replicate;
use lightyear_sync::prelude::client::{Input, InputDelayConfig};
use lightyear_sync::prelude::InputTimeline;
use test_log::test;

/// Check that ActionStates are stored correctly in the InputBuffer
#[test]
fn test_buffer_inputs_with_delay() {
    let mut stepper = ClientServerStepper::single();
    stepper.client_mut(0).insert(InputTimeline(Timeline::from(
        Input::default().with_input_delay(InputDelayConfig::fixed_input_delay(1)),
    )));
    let mut actions = Actions::<BEIContext>::default();
    actions.bind::<BEIAction1>();

    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn((
            actions,
            bei::prelude::InputMarker::<BEIContext>::default(),
            Replicate::to_server(),
        ))
        .id();
    stepper.frame_step(2);
    let server_entity = stepper
        .server()
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity is not present in entity map");

    stepper.frame_step(1);

    // mock press on a key
     stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .get_mut::<Actions<BEIContext>>()
        .unwrap()
        .mock::<BEIAction1>(ActionState::Fired, true, MockSpan::Updates(1));
    stepper.frame_step(1);
    let client_tick = stepper.client_tick(0);

    // check that the action state got buffered without any press (because the input is delayed)
    // (we cannot use JustPressed because we start by ticking the ActionState)
    // (i.e. the InputBuffer is empty for the current tick, and has the button press only with 1 tick of delay)
    let get_actions_for_tick = |tick: Tick, app: &mut App| {
        let world = app.world_mut().as_unsafe_world_cell();
        let world_clone = world.clone();

        let world = unsafe { world.world_mut() };
        let world_clone = unsafe { world_clone.world_mut() };
        let mut system_state : SystemState<Res<bei::prelude::InputRegistry>> = SystemState::new(world);
        let registry = system_state.get_mut(world);
        let snapshot = world_clone
                .entity(client_entity)
                .get::<InputBuffer<bei::input_message::ActionsSnapshot>>()
                .unwrap()
                .get(client_tick)
                .unwrap();
        let mut actions = Actions::<BEIContext>::default();
        BEIStateSequence::from_snapshot(&mut actions, snapshot, &registry);
        actions
    };

    let actions = get_actions_for_tick(client_tick, stepper.client_app());
    assert_eq!(
        actions.state::<BEIAction1>().unwrap(), ActionState::None
    );
    // if we check the next tick (delay of 1), we can see that the InputBuffer contains the ActionState with a press
    let actions = get_actions_for_tick(client_tick + 1, stepper.client_app());
    assert_eq!(
         actions.state::<BEIAction1>().unwrap(), ActionState::Fired
    );

    // mock release the key
     stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .get_mut::<Actions<BEIContext>>()
        .unwrap()
        .mock::<BEIAction1>(ActionState::None, false, MockSpan::Updates(1));

    // TODO: ideally we would check that the value of the ActionState inside FixedUpdate is correct
    // step another frame, this time we get the buffered input from earlier
    stepper.frame_step(1);
    let actions = get_actions_for_tick(client_tick + 1, stepper.client_app());
    assert_eq!(
        actions.state::<BEIAction1>().unwrap(), ActionState::Fired
    );
    let actions = get_actions_for_tick(client_tick + 2, stepper.client_app());
    assert_eq!(
        actions.state::<BEIAction1>().unwrap(), ActionState::None
    );

    // TODO: instead of just swapping the ActionState with the
    // the fixed_update_state ActionState outside of FixedUpdate is the delayed one
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(client_entity)
            .get::<Actions<BEIContext>>()
            .unwrap()
            .state::<BEIAction1>()
            .unwrap(),
        ActionState::None
    );
}
