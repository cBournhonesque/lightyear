use crate::protocol::NativeInput as MyInput;
use crate::stepper::{ClientServerStepper, TICK_DURATION};
use bevy::prelude::*;
use lightyear::input::native::prelude::InputMarker;
use lightyear::input::prelude::InputBuffer;
use lightyear::prelude::NetworkTimeline;
use lightyear::prelude::input::native::ActionState;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_core::prelude::{LocalTimeline, Rollback};
use lightyear_link::Link;
use lightyear_link::prelude::LinkConditionerConfig;
use lightyear_messages::MessageManager;
use lightyear_prediction::prelude::PredictionManager;
use lightyear_replication::components::Confirmed;
use lightyear_replication::prelude::{PredictionTarget, Replicate};
use lightyear_sync::prelude::InputTimeline;
use lightyear_sync::prelude::client::IsSynced;
use test_log::test;
use tracing::info;

/// Test a remote client's replicated entity sending inputs to the server
#[test]
fn test_remote_client_replicated_input() {
    let mut stepper = ClientServerStepper::single();

    stepper
        .client_app()
        .world_mut()
        .query::<&IsSynced<InputTimeline>>()
        .single(stepper.client_app().world())
        .unwrap();

    // SETUP
    // entity controlled by the remote client
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn(Replicate::to_clients(NetworkTarget::All))
        .id();

    stepper.frame_step(2);

    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity was not replicated to client");

    // TEST
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_entity)
        .insert(InputMarker::<MyInput>::default());
    stepper
        .client_app()
        .world_mut()
        .get_mut::<ActionState<MyInput>>(client_entity)
        .unwrap()
        .0 = MyInput(1);

    stepper.frame_step(1);
    let server_tick = stepper.server_tick();
    let client_tick = stepper.client_tick(0);

    // Client send an InputMessage to the server, who then adds an InputBuffer.
    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<InputBuffer<ActionState<MyInput>>>(server_entity)
            .unwrap()
            .get(client_tick)
            .unwrap(),
        &ActionState(MyInput(1))
    );

    // Advance to client tick to verify server applies the input
    stepper.frame_step((client_tick.0 - server_tick.0) as usize);

    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<ActionState<MyInput>>(server_entity)
            .unwrap(),
        &ActionState(MyInput(1))
    );
}

/// Test a remote client's predicted entity sending inputs to the server
#[test]
fn test_remote_client_predicted_input() {
    let mut stepper = ClientServerStepper::single();

    // SETUP
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();

    stepper.frame_step(2);

    let client_confirmed = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity was not replicated to client");

    let client_predicted = stepper
        .client_app()
        .world()
        .get::<Confirmed>(client_confirmed)
        .unwrap()
        .predicted
        .unwrap();
    info!(?client_predicted, ?client_confirmed, "client entities");

    // TEST
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_predicted)
        .insert(InputMarker::<MyInput>::default());
    stepper
        .client_app()
        .world_mut()
        .get_mut::<ActionState<MyInput>>(client_predicted)
        .unwrap()
        .0 = MyInput(2);

    stepper.frame_step(1);
    let server_tick = stepper.server_tick();
    let client_tick = stepper.client_tick(0);

    // ASSERT
    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<InputBuffer<ActionState<MyInput>>>(server_entity)
            .unwrap()
            .get(client_tick)
            .unwrap(),
        &ActionState(MyInput(2))
    );

    // Advance to client tick to verify server applies the input
    stepper.frame_step((client_tick.0 - server_tick.0) as usize);

    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<ActionState<MyInput>>(server_entity)
            .unwrap(),
        &ActionState(MyInput(2))
    );
}

/// Test a remote client's confirmed entity sending inputs to the server
#[test]
fn test_remote_client_confirmed_input() {
    let mut stepper = ClientServerStepper::single();

    // SETUP
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
        ))
        .id();

    stepper.frame_step(2);

    let client_confirmed = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity was not replicated to client");

    // TEST
    stepper
        .client_app()
        .world_mut()
        .entity_mut(client_confirmed)
        .insert(InputMarker::<MyInput>::default());
    stepper
        .client_app()
        .world_mut()
        .get_mut::<ActionState<MyInput>>(client_confirmed)
        .unwrap()
        .0 = MyInput(3);

    stepper.frame_step(1);
    let server_tick = stepper.server_tick();
    let client_tick = stepper.client_tick(0);

    // ASSERT
    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<InputBuffer<ActionState<MyInput>>>(server_entity)
            .unwrap()
            .get(client_tick)
            .unwrap(),
        &ActionState(MyInput(3))
    );

    // Advance to client tick to verify server applies the input
    stepper.frame_step((client_tick.0 - server_tick.0) as usize);

    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<ActionState<MyInput>>(server_entity)
            .unwrap(),
        &ActionState(MyInput(3))
    );
}

/// Test remote client inputs: we should be using the last known input value of the remote client, for better prediction accuracy!
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
            ActionState::<MyInput>::default(),
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

    // Add input markers to client 0, and make sure that it's replicated to client 1
    let client0_tick = stepper.client_tick(0);
    let client1_tick = stepper.client_tick(1);
    info!(?client0_tick, ?client1_tick, "Add input marker on client 0");
    stepper.client_apps[0]
        .world_mut()
        .entity_mut(client0_predicted)
        .insert(InputMarker::<MyInput>::default());
    stepper.frame_step(5);

    assert!(
        stepper.client_apps[1]
            .world()
            .get::<InputBuffer<ActionState<MyInput>>>(client1_predicted)
            .is_some()
    );

    // TEST SCENARIO:
    // Client 0 sends value 10
    stepper.client_apps[0]
        .world_mut()
        .get_mut::<ActionState<MyInput>>(client0_predicted)
        .unwrap()
        .0 = MyInput(10);
    stepper.frame_step(1);
    let client1_tick = stepper.client_tick(1);

    // switch to a different input value so distinction is clear
    stepper.client_apps[0]
        .world_mut()
        .get_mut::<ActionState<MyInput>>(client0_predicted)
        .unwrap()
        .0 = MyInput(5);

    // Step 2 more frames because the input is delayed. And because we need client0 -> server -> client1
    // the client 1 should just have received the Input(10)
    stepper.frame_step_server_first(2);

    // make sure that the client 1 receives them
    let buffer = stepper.client_apps[1]
        .world()
        .get::<InputBuffer<ActionState<MyInput>>>(client1_predicted)
        .expect("input buffer should exist");
    info!(?buffer, "client 1 tick: {:?}", client1_tick);
    assert_eq!(buffer.end_tick().unwrap(), client1_tick);
    // make sure that the ActionState on client 1's predicted entity has been updated
    let action_state = stepper.client_apps[1]
        .world()
        .get::<ActionState<MyInput>>(client1_predicted)
        .expect("action state should exist");
    assert_eq!(action_state.0, MyInput(10));

    stepper.frame_step_server_first(2);
    // check that the next input has been received
    let action_state = stepper.client_apps[1]
        .world()
        .get::<ActionState<MyInput>>(client1_predicted)
        .expect("action state should exist");
    assert_eq!(action_state.0, MyInput(5));

    // check that during rollbacks, we fetch the input value from the input buffer even for remote inputs
    let check_input =
        move |c: Single<&LocalTimeline, With<Rollback>>,
              q: Single<&ActionState<MyInput>, Without<InputMarker<MyInput>>>| {
            info!(
                "Checking input value {:?} during rollback. Tick: {:?}",
                q.0,
                c.tick()
            );
            if c.tick() == client1_tick {
                info!("checking that we fetch old value from the buffer");
                assert_eq!(
                    q.0,
                    MyInput(10),
                    "During rollback, we should fetch the ActionState from the buffer",
                );
            }
            if c.tick() == client1_tick + 3 {
                info!(
                    "checking that the action state stays correct for ticks for which we don't have an input in the buffer. (we predict that it stays the same)"
                );
                assert_eq!(
                    q.0,
                    MyInput(5),
                    "During rollback, we should fetch the ActionState from the buffer",
                );
            }
        };

    stepper.client_apps[1].add_systems(FixedUpdate, check_input);

    // trigger rollback for client 1
    let rollback_tick = client1_tick - 1;
    stepper.client_mut(1).insert(Rollback::FromInputs);
    stepper
        .client_mut(1)
        .get_mut::<PredictionManager>()
        .unwrap()
        .set_rollback_tick(rollback_tick);
    stepper.client_apps[1].update();
}

// /// Test a remote client's pre-predicted entity sending inputs to the server
// #[test]
// fn test_remote_client_prepredicted_entity_input() {
//     let mut stepper = ClientServerStepper::default();
//
//     // SETUP
//     let client_pre_predicted_entity = stepper
//         .client_app
//         .world_mut()
//         .spawn((client::Replicate::default(), PrePredicted::default()))
//         .id();
//
//     for _ in 0..10 {
//         stepper.frame_step();
//     }
//
//     let server_pre_predicted_entity = stepper
//         .server_app
//         .world_mut()
//         .query_filtered::<Entity, With<PrePredicted>>()
//         .single(stepper.server_app.world())
//         .unwrap();
//
//     // Replicate back the pre-predicted entity
//     stepper
//         .server_app
//         .world_mut()
//         .entity_mut(server_pre_predicted_entity)
//         .insert(Replicate::default());
//
//     stepper.frame_step();
//     stepper.frame_step();
//
//     // TEST
//     stepper
//         .client_app
//         .world_mut()
//         .entity_mut(client_pre_predicted_entity)
//         .insert(InputMarker::<MyInput>::default());
//     stepper
//         .client_app
//         .world_mut()
//         .get_mut::<ActionState<MyInput>>(client_pre_predicted_entity)
//         .unwrap()
//         .value = Some(MyInput(4));
//
//     stepper.frame_step();
//     let server_tick = stepper.server_tick();
//     let client_tick = stepper.client_tick();
//
//     // ASSERT
//     assert_eq!(
//         stepper
//             .server_app
//             .world()
//             .get::<InputBuffer<ActionState<MyInput>>>(server_pre_predicted_entity)
//             .unwrap()
//             .get(client_tick)
//             .unwrap(),
//         &ActionState {
//             value: Some(MyInput(4))
//         }
//     );
//
//     // Advance to client tick to verify server applies the input
//     for tick in (server_tick.0 as usize)..(client_tick.0 as usize) {
//         stepper.frame_step();
//     }
//
//     assert_eq!(
//         stepper
//             .server_app
//             .world()
//             .get::<ActionState<MyInput>>(server_pre_predicted_entity)
//             .unwrap(),
//         &ActionState {
//             value: Some(MyInput(4))
//         }
//     );
// }

// /// Test local client inputs being sent to the server
// #[test]
// fn test_local_client_input_to_server() {
//     // Note: In a host-server mode, local client inputs are automatically
//     // applied to the server as they share the same process
//     let mut stepper = ClientServerStepper::default();
//
//     // SETUP
//     // Entity controlled by the local client
//     let local_entity = stepper
//         .server_app
//         .world_mut()
//         .spawn((
//             Replicate {
//                 sync: SyncTarget {
//                     prediction: NetworkTarget::All,
//                     ..default()
//                 },
//                 ..default()
//             },
//             InputMarker::<MyInput>::default(),
//         ))
//         .id();
//
//     for _ in 0..10 {
//         stepper.frame_step();
//     }
//
//     // TEST
//     // In host-server mode, we can directly set the input on the server entity
//     // and it will be used in the next frame
//     stepper
//         .server_app
//         .world_mut()
//         .get_mut::<ActionState<MyInput>>(local_entity)
//         .unwrap()
//         .value = Some(MyInput(5));
//
//     stepper.frame_step();
//
//     // ASSERT
//     assert_eq!(
//         stepper
//             .server_app
//             .world()
//             .get::<ActionState<MyInput>>(local_entity)
//             .unwrap(),
//         &ActionState {
//             value: Some(MyInput(5))
//         }
//     );
// }
//
// /// Test local host-server client inputs being sent to remote client for prediction
// #[test]
// fn test_local_client_input_for_prediction() {
//     let mut stepper = ClientServerStepper::default();
//
//     // SETUP
//     // Entity controlled by the local client
//     let local_entity = stepper
//         .server_app
//         .world_mut()
//         .spawn((
//             Replicate {
//                 sync: SyncTarget {
//                     prediction: NetworkTarget::All,
//                     ..default()
//                 },
//                 ..default()
//             },
//             InputMarker::<MyInput>::default(),
//         ))
//         .id();
//
//     for _ in 0..10 {
//         stepper.frame_step();
//     }
//
//     let local_confirmed = stepper
//         .client_app
//         .world()
//         .resource::<client::ConnectionManager>()
//         .replication_receiver
//         .remote_entity_map
//         .get_local(local_entity)
//         .expect("entity was not replicated to client");
//
//     let local_predicted = stepper
//         .client_app
//         .world()
//         .get::<Confirmed>(local_confirmed)
//         .unwrap()
//         .predicted
//         .unwrap();
//
//     // TEST
//     // Set input on the server entity which should be broadcasted to clients
//     stepper
//         .server_app
//         .world_mut()
//         .get_mut::<ActionState<MyInput>>(local_entity)
//         .unwrap()
//         .value = Some(MyInput(6));
//
//     // Run server first, then client, so the server's rebroadcasted inputs can be read by the client
//     stepper.advance_time(stepper.frame_duration);
//     stepper.server_app.update();
//     stepper.client_app().update();
//
//     let server_tick = stepper.server_tick();
//
//     // ASSERT
//     // For input broadcasting, we write the remote client inputs to the Predicted entity only
//     assert_eq!(
//         stepper
//             .client_app
//             .world()
//             .get::<InputBuffer<ActionState<MyInput>>>(local_predicted)
//             .unwrap()
//             .get(server_tick)
//             .unwrap(),
//         &ActionState {
//             value: Some(MyInput(6))
//         }
//     );
// }
