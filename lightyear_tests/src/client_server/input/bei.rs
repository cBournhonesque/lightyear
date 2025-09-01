use crate::client_server::prediction::trigger_state_rollback;
use crate::protocol::{BEIAction1, BEIContext};
use crate::stepper::{ClientServerStepper, TICK_DURATION};
use bevy::app::{App, FixedPostUpdate};
use bevy::ecs::relationship::Relationship;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::*;
use lightyear::input::bei;
use lightyear::input::bei::input_message::{ActionData, ActionsSnapshot, BEIStateSequence};
use lightyear::input::input_buffer::InputBuffer;
use lightyear::input::input_message::ActionStateQueryData;
use lightyear::input::input_message::ActionStateSequence;
use lightyear::input::native::prelude::InputMarker;
use lightyear_connection::client::Client;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline, Rollback, Tick, Timeline};
use lightyear_link::Link;
use lightyear_link::prelude::LinkConditionerConfig;
use lightyear_messages::MessageManager;
use lightyear_prediction::diagnostics::PredictionMetrics;
use lightyear_prediction::manager::PredictionManager;
use lightyear_prediction::prelude::DeterministicPredicted;
use lightyear_replication::components::Confirmed;
use lightyear_replication::prelude::{PredictionTarget, Replicate};
use lightyear_sync::prelude::InputTimeline;
use lightyear_sync::prelude::client::{Input, InputDelayConfig};
use test_log::test;
use tracing::info;

/// Check that we can insert actions on the client entity
#[test]
fn test_actions_on_client_entity() {
    let mut stepper = ClientServerStepper::single();
    // we spawn an action entity on the client
    let client_entity = stepper.client(0).id();
    let client_action = stepper
        .client_app()
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(client_entity),
            Action::<BEIAction1>::default(),
        ))
        .id();
    stepper.frame_step(1);
    // Add an InputMarker to the Context entity on the client
    // to indicate that the client controls this entity
    // (it gets propagated to the Action entity)
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert((
            BEIContext,
            bei::prelude::InputMarker::<BEIContext>::default(),
        ));

    let server_action = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_action)
        .expect("entity is not present in entity map");
    let client_of_entity = stepper.client_of(0).id();
    // Check that the ActionOf component was mapped correctly on the server
    // (i.e the context entity is the Client on the client, and the ClientOf on the server)
    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_action)
            .get::<ActionOf<BEIContext>>()
            .unwrap()
            .get(),
        client_of_entity
    );

    info!("Mocking press on BEIAction1");
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_action)
        .insert(ActionMock::once(ActionState::Fired, true));
    stepper.frame_step(1);
    let client_tick = stepper.client_tick(0);

    let snapshot = stepper
        .server_app
        .world()
        .entity(server_action)
        .get::<InputBuffer<ActionsSnapshot<BEIContext>>>()
        .unwrap()
        .get(client_tick)
        .cloned()
        .unwrap_or(ActionsSnapshot::<BEIContext>::default());
    let mut actions = ActionData::base_value();
    BEIStateSequence::from_snapshot(ActionData::as_mut(&mut actions), &snapshot);
    // check that we received the snapshot on the server
    assert_eq!(actions.0, ActionState::Fired);
}

/// Check that ActionStates are stored correctly in the InputBuffer
#[test]
fn test_buffer_inputs_with_delay() {
    let mut stepper = ClientServerStepper::single();
    stepper.client_mut(0).insert(InputTimeline(Timeline::from(
        Input::default().with_input_delay(InputDelayConfig::fixed_input_delay(1)),
    )));
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((BEIContext, Replicate::to_clients(NetworkTarget::All)))
        .id();
    stepper.frame_step(2);

    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");

    // we spawn an action entity on the client
    let client_action = stepper
        .client_app()
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(client_entity),
            Action::<BEIAction1>::default(),
        ))
        .id();

    // check that the Action entity contains Replicate
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_action)
            .contains::<Replicate>()
    );

    stepper.frame_step(1);

    // Add an InputMarker to the Context entity on the client to indicate that the client controls this entity
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(bei::prelude::InputMarker::<BEIContext>::default());

    // check that the InputMarker was propagated to the Action entity
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_action)
            .contains::<bei::prelude::InputMarker<BEIContext>>()
    );
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_action)
            .contains::<InputBuffer<ActionsSnapshot<BEIContext>>>()
    );
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_action)
            .contains::<ActionState>()
    );
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_action)
            .contains::<ActionTime>()
    );

    let server_action = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_action)
        .expect("entity is not present in entity map");

    // Check that the server entity also has the Action component
    assert!(
        stepper
            .server_app
            .world()
            .entity(server_action)
            .contains::<Action<BEIAction1>>()
    );

    // mock press on a key
    info!("Mocking press on BEIAction1");
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_action)
        .insert(ActionMock::once(ActionState::Fired, true));
    stepper.frame_step(1);
    let client_tick = stepper.client_tick(0);

    // check that the action state got buffered without any press (because the input is delayed)
    // (we cannot use JustPressed because we start by ticking the ActionState)
    // (i.e. the InputBuffer is empty for the current tick, and has the button press only with 1 tick of delay)
    let get_action_state_for_tick = |tick: Tick, app: &mut App| {
        let world = app.world_mut();
        let snapshot = world
            .entity(client_action)
            .get::<InputBuffer<ActionsSnapshot<BEIContext>>>()
            .unwrap()
            .get(tick)
            .cloned()
            .unwrap_or(ActionsSnapshot::<BEIContext>::default());
        let mut actions = ActionData::base_value();
        BEIStateSequence::from_snapshot(ActionData::as_mut(&mut actions), &snapshot);
        actions.0
    };

    let action_state = get_action_state_for_tick(client_tick, stepper.client_app());
    assert_eq!(action_state, ActionState::None);
    // if we check the next tick (delay of 1), we can see that the InputBuffer contains the ActionState with a press
    let action_state = get_action_state_for_tick(client_tick + 1, stepper.client_app());
    assert_eq!(action_state, ActionState::Fired);

    // mock release the key
    info!("Mocking release on BEIAction1");
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_action)
        .insert(ActionMock::once(ActionState::None, true));

    // TODO: ideally we would check that the value of the ActionState inside FixedUpdate is correct
    // step another frame, this time we get the buffered input from earlier
    stepper.frame_step(1);
    let action_state = get_action_state_for_tick(client_tick + 1, stepper.client_app());
    assert_eq!(action_state, ActionState::Fired);
    let action_state = get_action_state_for_tick(client_tick + 2, stepper.client_app());
    assert_eq!(action_state, ActionState::None);

    // TODO: instead of just swapping the ActionState with the
    // the fixed_update_state ActionState outside of FixedUpdate is the delayed one
    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(client_action)
            .get::<ActionState>()
            .unwrap(),
        &ActionState::None
    );
}

/// Check that Actions<C> is restored correctly after a rollback, including timing
/// information
#[test]
fn test_client_rollback() {
    let mut stepper = ClientServerStepper::single();

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((BEIContext, Replicate::to_clients(NetworkTarget::All)))
        .id();
    stepper.frame_step(2);

    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity is not present in entity map");

    // we spawn an action entity on the client
    let client_action = stepper
        .client_app()
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(client_entity),
            Action::<BEIAction1>::default(),
        ))
        .id();

    // check that the Action entity contains Replicate
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_action)
            .contains::<Replicate>()
    );

    stepper.frame_step(1);

    // Add an InputMarker to the Context entity on the client to indicate that the client controls this entity
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(bei::prelude::InputMarker::<BEIContext>::default());

    let server_action = stepper
        .server_app
        .world()
        .entity(server_entity)
        .get::<Actions<BEIContext>>()
        .unwrap()
        .collection()[0];

    // mock press on a key
    info!("Mocking press on BEIAction1 for 3 ticks");
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_action)
        .insert(ActionMock::new(
            ActionState::Fired,
            true,
            MockSpan::Updates(3),
        ));
    // first tick: we start pressing the button, the elapsed time is 0.0 (because it corresponds to the previous action)
    // second tick: the action is pressed, the elapsed time is 0.1 (because it was pressed for 0.1)
    // third tick: the action is pressed, the elapsed time is 0.2 (because it was pressed for 0.2)
    stepper.frame_step(3);
    let client_tick = stepper.client_tick(0);

    let client_action_ref = stepper.client_app().world().entity(client_action);
    let action_time = client_action_ref.get::<ActionTime>().unwrap();
    // the duration starts incrementing from the second tick after the action is pressed
    assert_eq!(action_time.fired_secs, TICK_DURATION.as_secs_f32() * 2.0);
    stepper.frame_step(2);
    let action_state = stepper
        .client_app()
        .world()
        .entity(client_action)
        .get::<ActionState>()
        .unwrap();
    assert_eq!(action_state, &ActionState::None);

    // trigger a rollback
    // at client_tick, the elapsed_time should be 0.2.
    // We rollback to client_tick - 1, because the first FixedPreUpdate will bring us to `client_tick`
    trigger_state_rollback(&mut stepper, client_tick - 1);

    let assert_action_duration =
        move |client: Single<&LocalTimeline, With<Client>>, query: Single<&ActionTime>| {
            let tick = client.tick();
            if tick == client_tick {
                let action_time = query.into_inner();
                assert_eq!(action_time.fired_secs, TICK_DURATION.as_secs_f32() * 2.0);
            }
        };
    stepper
        .client_app()
        .add_systems(FixedPostUpdate, assert_action_duration);

    // Do the rollback
    stepper.frame_step(1);
}

/// Test remote client inputs: we should be using the last known input value of the remote client, for better prediction accuracy!
/// Then for the missing ticks we should be predicting the future value of the inpu
///
/// Also checks that during rollbacks we fetch the correct input value even for remote inputs.
///
/// For example if we receive inputs from client 1 with 5 tick delay, then when we are tick 35 we receive
/// the input for tick 30. In that case we should either:
/// - launch a rollback check immediately for tick 30
/// - or at least at tick 35 use the newly received input value for prediction!
#[test]
fn test_input_broadcasting_prediction() {
    let mut stepper = ClientServerStepper::with_clients(2);
    let server_recv_delay: i16 = 2;

    // client 0 has some latency to send inputs to the server
    stepper
        .client_of_mut(0)
        .get_mut::<Link>()
        .unwrap()
        .recv
        .conditioner = Some(lightyear_link::LinkConditioner::new(
        LinkConditionerConfig {
            incoming_latency: TICK_DURATION * (server_recv_delay as u32),
            ..default()
        },
    ));

    // SETUP - Create an entity controlled by client 0, predicted by all clients
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            BEIContext,
        ))
        .id();
    stepper.frame_step_server_first(1);

    // Get the predicted entities on both clients
    let client0_confirmed = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity not replicated to client 0");

    let client0_predicted = stepper.client_apps[0]
        .world()
        .get::<Confirmed>(client0_confirmed)
        .unwrap()
        .predicted
        .unwrap();
    // we spawn an action entity on the client
    // Add input markers to client 0, and make sure that it's replicated to client 1
    let client0_tick = stepper.client_tick(0);
    let client1_tick = stepper.client_tick(1);
    info!(
        ?server_entity,
        ?client0_confirmed,
        ?client0_predicted,
        ?client0_tick,
        ?client1_tick,
        "Add input marker on client 0"
    );
    let client_action = stepper.client_apps[0]
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(client0_predicted),
            Action::<BEIAction1>::default(),
            ActionMock::new(
                ActionState::Fired,
                ActionValue::Bool(true),
                MockSpan::Manual,
            ),
        ))
        .id();

    let client1_confirmed = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity not replicated to client 1");

    let client1_predicted = stepper.client_apps[1]
        .world()
        .get::<Confirmed>(client1_confirmed)
        .unwrap()
        .predicted
        .unwrap();

    stepper.frame_step(5);
    // client0 + 1: client 0 sends the input (with 2 ticks delay)
    // client0 + 3: server receives the input (with 2 ticks delay) and rebroadcasts it to client 1
    // client1 + 4: client 1 receives the input but cannot process it yet because it receives the input BEFORE it spawns the rebroadcasted Action entity
    // client1 + 5: client 1 checks rollback for tick (client1 + 4), there is a rollback because of mismatch.

    let action1 = stepper.client_apps[1]
        .world()
        .get::<Actions<BEIContext>>(client1_predicted)
        .unwrap()
        .collection()[0];
    assert!(
        stepper.client_apps[1]
            .world()
            .entity(action1)
            .get::<InputBuffer<ActionsSnapshot<BEIContext>>>()
            .is_some()
    );

    // TEST:
    // check that on the last frame, client1 processed the rebroadcasted inputs
    // - it should update its buffer to match the remote message
    // - it should trigger a rollback because of mismatch
    assert_eq!(
        stepper.client_apps[1]
            .world()
            .entity(action1)
            .get::<InputBuffer<ActionsSnapshot<BEIContext>>>()
            .unwrap()
            .get(client1_tick + 1)
            .unwrap(),
        &ActionsSnapshot::<BEIContext>::new(
            ActionState::Fired,
            ActionValue::Bool(true),
            ActionTime::default(),
            ActionEvents::STARTED | ActionEvents::FIRED
        )
    );
    assert_eq!(
        stepper.client_apps[1]
            .world()
            .entity(action1)
            .get::<InputBuffer<ActionsSnapshot<BEIContext>>>()
            .unwrap()
            .get(client1_tick + 2)
            .unwrap(),
        &ActionsSnapshot::<BEIContext>::new(
            ActionState::Fired,
            ActionValue::Bool(true),
            ActionTime {
                elapsed_secs: 0.01,
                fired_secs: 0.01
            },
            ActionEvents::FIRED
        )
    );
    // check that a rollback was triggered on client 1
    assert_eq!(
        stepper.client_apps[1]
            .world()
            .get_resource::<PredictionMetrics>()
            .unwrap()
            .rollbacks,
        1
    );

    stepper.frame_step(1);

    // check that the input buffer is still correct after receiving a new remote input
    assert_eq!(
        stepper.client_apps[1]
            .world()
            .entity(action1)
            .get::<InputBuffer<ActionsSnapshot<BEIContext>>>()
            .unwrap()
            .get(client1_tick + 3)
            .unwrap(),
        &ActionsSnapshot::<BEIContext>::new(
            ActionState::Fired,
            ActionValue::Bool(true),
            ActionTime {
                elapsed_secs: 0.02,
                fired_secs: 0.02
            },
            ActionEvents::FIRED
        )
    );
    // check that this time there was no new rollback since we predicted the correct input value
    assert_eq!(
        stepper.client_apps[1]
            .world()
            .get_resource::<PredictionMetrics>()
            .unwrap()
            .rollbacks,
        1
    );
}
