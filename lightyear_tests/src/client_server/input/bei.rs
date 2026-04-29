use crate::client_server::prediction::trigger_state_rollback;
use crate::protocol::{BEIAction1, BEIContext};
use crate::stepper::*;
use bevy::app::{App, FixedPostUpdate, FixedPreUpdate};
use bevy::ecs::relationship::Relationship;
use bevy::prelude::*;
use bevy_enhanced_input::action::TriggerState;
use bevy_enhanced_input::prelude::*;
use lightyear::input::bei;
use lightyear::input::bei::input_message::{ActionData, ActionsSnapshot, BEIStateSequence};
use lightyear::input::bei::prelude::BEIBuffer;
use lightyear::input::input_message::ActionStateQueryData;
use lightyear::input::input_message::ActionStateSequence;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::prelude::*;
use lightyear_link::Link;
use lightyear_link::prelude::LinkConditionerConfig;
use lightyear_messages::MessageManager;
use lightyear_prediction::diagnostics::PredictionMetrics;
use lightyear_replication::prelude::{PreSpawned, PredictionTarget, Replicate};
use lightyear_sync::prelude::client::{InputDelayConfig, InputTimelineConfig};
use test_log::test;
use tracing::info;

const TEST_HASH: u64 = 42;

#[derive(Resource, Default)]
struct SawServerFiredAction(bool);

fn record_server_fired_action(
    timeline: Res<LocalTimeline>,
    query: Query<&BEIBuffer<BEIContext>>,
    mut saw: ResMut<SawServerFiredAction>,
) {
    if saw.0 {
        return;
    }

    let tick = timeline.tick();
    saw.0 = query.iter().any(|buffer| {
        (0..=1).any(|offset| {
            let sample_tick = tick - offset;
            buffer
                .get(sample_tick)
                .is_some_and(|snapshot| snapshot.state == TriggerState::Fired)
        })
    });
}

/// Helper: spawn an action entity on both client and server with PreSpawned matching.
/// Returns (client_action, server_action).
fn spawn_action_pair(
    stepper: &mut ClientServerStepper,
    client_context: Entity,
    server_context: Entity,
    hash: u64,
) -> (Entity, Entity) {
    let server_action = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(server_context),
            Action::<BEIAction1>::default(),
            PreSpawned::new(hash),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();
    let client_action = stepper
        .client_app()
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(client_context),
            Action::<BEIAction1>::default(),
            PreSpawned::new(hash),
        ))
        .id();
    (client_action, server_action)
}

/// Check that we can insert actions on the client entity using PreSpawned
#[test]
fn test_actions_on_client_entity() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let client_entity = stepper.client(0).id();
    let client_of_entity = stepper.client_of(0).id();

    let (client_action, server_action) =
        spawn_action_pair(&mut stepper, client_entity, client_of_entity, TEST_HASH);
    stepper.frame_step(1);

    // Add an InputMarker to the Context entity on the client
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert((
            BEIContext,
            bei::prelude::InputMarker::<BEIContext>::default(),
        ));

    // Check that the ActionOf component points to the correct entity on the server
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
        .insert(ActionMock::once(TriggerState::Fired, true));
    stepper.frame_step(1);
    let client_tick = stepper.client_tick(0);

    let snapshot = stepper
        .server_app
        .world()
        .entity(server_action)
        .get::<BEIBuffer<BEIContext>>()
        .unwrap()
        .get(client_tick)
        .cloned()
        .unwrap_or(ActionsSnapshot::default());
    let mut actions = ActionData::base_value();
    BEIStateSequence::<BEIContext>::from_snapshot(ActionData::as_mut(&mut actions), &snapshot);
    assert_eq!(actions.0, TriggerState::Fired);
}

/// Check that a client-spawned action entity for a received context maps correctly
/// when using PreSpawned
#[test]
fn test_action_spawned_from_received_context_maps_back_to_server_entity() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((BEIContext, Replicate::to_clients(NetworkTarget::All)))
        .id();

    // Also spawn the action entity on the server
    let server_action = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(server_entity),
            Action::<BEIAction1>::default(),
            PreSpawned::new(TEST_HASH),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();

    stepper.frame_step(3);

    // On the client, spawn the matching action entity when the context arrives
    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("context entity should be replicated to client");

    let client_action = stepper
        .client_app()
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(client_entity),
            Action::<BEIAction1>::default(),
            PreSpawned::new(TEST_HASH),
        ))
        .id();

    // Let the prespawn matching happen
    stepper.frame_step(2);

    assert_eq!(
        stepper
            .server_app
            .world()
            .entity(server_action)
            .get::<ActionOf<BEIContext>>()
            .unwrap()
            .get(),
        server_entity,
        "the server action must point to the server context"
    );
}

/// Check that a bound action entity sends inputs after PreSpawned matching
#[test]
fn test_bound_action_spawned_from_received_context_sends_inputs_after_mapping() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    stepper.server_app.init_resource::<SawServerFiredAction>();
    stepper.server_app.add_systems(
        FixedPreUpdate,
        record_server_fired_action
            .before(lightyear::input::server::InputSystems::UpdateActionState),
    );

    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((BEIContext, Replicate::to_clients(NetworkTarget::All)))
        .id();

    let server_action = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(server_entity),
            Action::<BEIAction1>::default(),
            PreSpawned::new(TEST_HASH),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();

    stepper.frame_step(3);

    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("context entity should be replicated to client");

    let client_action = stepper
        .client_app()
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(client_entity),
            Action::<BEIAction1>::default(),
            ActionMock::once(TriggerState::Fired, true),
            bindings![KeyCode::Space,],
            PreSpawned::new(TEST_HASH),
            bei::prelude::InputMarker::<BEIContext>::default(),
        ))
        .id();

    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(bei::prelude::InputMarker::<BEIContext>::default());

    stepper.frame_step(5);

    assert!(
        stepper
            .server_app
            .world()
            .resource::<SawServerFiredAction>()
            .0,
        "server should eventually receive the fired action state via input messages"
    );
}

/// Check that ActionStates are stored correctly in the InputBuffer with PreSpawned
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

    let client_of_entity = stepper.client_of(0).id();

    // Spawn action entities on both sides with PreSpawned
    let server_action = stepper
        .server_app
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(server_entity),
            Action::<BEIAction1>::default(),
            PreSpawned::new(TEST_HASH),
            Replicate::to_clients(NetworkTarget::All),
        ))
        .id();

    let client_action = stepper
        .client_app()
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(client_entity),
            Action::<BEIAction1>::default(),
            PreSpawned::new(TEST_HASH),
        ))
        .id();

    // With PreSpawned, the action entity should NOT have Replicate
    assert!(
        !stepper
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
            .contains::<BEIBuffer<BEIContext>>()
    );
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_action)
            .contains::<TriggerState>()
    );
    assert!(
        stepper
            .client_app()
            .world()
            .entity(client_action)
            .contains::<ActionTime>()
    );

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
        .insert(ActionMock::once(TriggerState::Fired, true));
    stepper.frame_step(1);
    let client_tick = stepper.client_tick(0);

    // check that the action state got buffered without any press (because the input is delayed)
    let get_action_state_for_tick = |tick: Tick, app: &mut App| {
        let world = app.world_mut();
        let snapshot = world
            .entity(client_action)
            .get::<BEIBuffer<BEIContext>>()
            .unwrap()
            .get(tick)
            .cloned()
            .unwrap_or(ActionsSnapshot::default());
        let mut actions = ActionData::base_value();
        BEIStateSequence::<BEIContext>::from_snapshot(ActionData::as_mut(&mut actions), &snapshot);
        actions.0
    };

    let action_state = get_action_state_for_tick(client_tick, stepper.client_app());
    assert_eq!(action_state, TriggerState::None);
    // if we check the next tick (delay of 1), we can see that the InputBuffer contains the ActionState with a press
    let action_state = get_action_state_for_tick(client_tick + 1, stepper.client_app());
    assert_eq!(action_state, TriggerState::Fired);

    // mock release the key
    info!("Mocking release on BEIAction1");
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_action)
        .insert(ActionMock::once(TriggerState::None, true));

    stepper.frame_step(1);
    let action_state = get_action_state_for_tick(client_tick + 1, stepper.client_app());
    assert_eq!(action_state, TriggerState::Fired);
    let action_state = get_action_state_for_tick(client_tick + 2, stepper.client_app());
    assert_eq!(action_state, TriggerState::None);

    assert_eq!(
        stepper
            .client_app()
            .world()
            .entity(client_action)
            .get::<TriggerState>()
            .unwrap(),
        &TriggerState::None
    );
}

/// Check that Actions<C> is restored correctly after a rollback
#[test]
fn test_client_rollback() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

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

    // Spawn action entities on both sides with PreSpawned
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

    let client_action = stepper
        .client_app()
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(client_entity),
            Action::<BEIAction1>::default(),
            PreSpawned::new(TEST_HASH),
        ))
        .id();

    assert!(
        !stepper
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

    // mock press on a key
    info!("Mocking press on BEIAction1 for 3 ticks");
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_action)
        .insert(ActionMock::new(
            TriggerState::Fired,
            true,
            MockSpan::Updates(3),
        ));
    stepper.frame_step(3);
    let client_tick = stepper.client_tick(0);

    let client_action_ref = stepper.client_app().world().entity(client_action);
    let action_time = client_action_ref.get::<ActionTime>().unwrap();
    assert_eq!(action_time.fired_secs, TICK_DURATION.as_secs_f32() * 2.0);
    stepper.frame_step(2);
    let action_state = stepper
        .client_app()
        .world()
        .entity(client_action)
        .get::<TriggerState>()
        .unwrap();
    assert_eq!(action_state, &TriggerState::None);

    // trigger a rollback
    trigger_state_rollback(&mut stepper, client_tick - 1);

    let assert_action_duration = move |timeline: Res<LocalTimeline>, query: Single<&ActionTime>| {
        let tick = timeline.tick();
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

/// Check that Actions<C> is restored correctly after a rollback, and observers are re-triggered
#[test]
fn test_client_rollback_bei_events() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());

    stepper.client_apps[0].init_resource::<Counter>();
    stepper.client_apps[0].add_observer(
        |trigger: On<bevy_enhanced_input::prelude::Start<BEIAction1>>,
         mut counter: ResMut<Counter>| counter.0 += 1,
    );

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

    // Spawn action entities on both sides with PreSpawned
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

    let client_action = stepper
        .client_app()
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(client_entity),
            Action::<BEIAction1>::default(),
            PreSpawned::new(TEST_HASH),
        ))
        .id();

    assert!(
        !stepper
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

    // mock press on a key
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_action)
        .insert(ActionMock::new(
            TriggerState::Fired,
            true,
            MockSpan::Updates(1),
        ));
    stepper.frame_step(3);
    let client_tick = stepper.client_tick(0);

    // Check that the START event got fired
    assert_eq!(stepper.client_app().world().resource::<Counter>().0, 1);

    // trigger a rollback
    trigger_state_rollback(&mut stepper, client_tick - 4);

    // Do the rollback
    stepper.frame_step(1);

    // check that the START event got fired again
    assert_eq!(stepper.client_app().world().resource::<Counter>().0, 2);
}

#[derive(Resource, Default)]
struct Counter(usize);

/// Test remote client inputs: we should be using the last known input value of the remote client, for better prediction accuracy!
/// Then for the missing ticks we should be predicting the future value of the input
///
/// Also checks that during rollbacks we fetch the correct input value even for remote inputs.
#[test]
fn test_input_broadcasting_prediction() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::with_netcode_clients(2));
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

    // Spawn the action entity on the server with PreSpawned
    let server_action = stepper
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
    let client0_predicted = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity not replicated to client 0");

    // Spawn matching action entity on client 0 with PreSpawned + input mock
    let client0_tick = stepper.client_tick(0);
    let client1_tick = stepper.client_tick(1);
    info!(
        ?server_entity,
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
                TriggerState::Fired,
                ActionValue::Bool(true),
                MockSpan::Manual,
            ),
            PreSpawned::new(TEST_HASH),
            bei::prelude::InputMarker::<BEIContext>::default(),
        ))
        .id();

    let client1_predicted = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity not replicated to client 1");

    stepper.frame_step(5);

    // Find the action entity on client 1 that has an InputBuffer
    let action1 = {
        let world = stepper.client_apps[1].world();
        let actions = world.get::<Actions<BEIContext>>(client1_predicted).unwrap();
        actions
            .collection()
            .iter()
            .copied()
            .find(|action| {
                world
                    .entity(*action)
                    .get::<BEIBuffer<BEIContext>>()
                    .is_some()
            })
            .unwrap_or(actions.collection()[0])
    };
    assert!(
        stepper.client_apps[1]
            .world()
            .entity(action1)
            .get::<BEIBuffer<BEIContext>>()
            .is_some()
    );

    // Check that on the last frame, client1 processed the rebroadcasted inputs
    let first_remote_tick = {
        let buffer = stepper.client_apps[1]
            .world()
            .entity(action1)
            .get::<BEIBuffer<BEIContext>>()
            .unwrap();
        [1, 2, 3, 4, 5, 6]
            .into_iter()
            .map(|offset| client1_tick + offset)
            .find(|tick| buffer.get(*tick).is_some())
            .unwrap()
    };
    assert_eq!(
        stepper.client_apps[1]
            .world()
            .entity(action1)
            .get::<BEIBuffer<BEIContext>>()
            .unwrap()
            .get(first_remote_tick)
            .unwrap(),
        &ActionsSnapshot {
            state: TriggerState::Fired,
            value: ActionValue::Bool(true),
            time: ActionTime::default(),
            events: ActionEvents::START | ActionEvents::FIRE
        }
    );
    assert_eq!(
        stepper.client_apps[1]
            .world()
            .entity(action1)
            .get::<BEIBuffer<BEIContext>>()
            .unwrap()
            .get(first_remote_tick + 1)
            .unwrap(),
        &ActionsSnapshot {
            state: TriggerState::Fired,
            value: ActionValue::Bool(true),
            time: ActionTime {
                elapsed_secs: 0.01,
                fired_secs: 0.01
            },
            events: ActionEvents::FIRE
        }
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
            .get::<BEIBuffer<BEIContext>>()
            .unwrap()
            .get(first_remote_tick + 2)
            .unwrap(),
        &ActionsSnapshot {
            state: TriggerState::Fired,
            value: ActionValue::Bool(true),
            time: ActionTime {
                elapsed_secs: 0.02,
                fired_secs: 0.02
            },
            events: ActionEvents::FIRE
        }
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
