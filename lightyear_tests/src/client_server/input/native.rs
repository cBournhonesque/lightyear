use crate::protocol::NativeInput as MyInput;
use crate::stepper::ClientServerStepper;
use lightyear::input::native::prelude::InputMarker;
use lightyear::input::prelude::InputBuffer;
use lightyear::prelude::input::native::ActionState;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
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
        .value = Some(MyInput(1));

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
        &ActionState {
            value: Some(MyInput(1))
        }
    );

    // Advance to client tick to verify server applies the input
    stepper.frame_step((client_tick.0 - server_tick.0) as usize);

    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<ActionState<MyInput>>(server_entity)
            .unwrap(),
        &ActionState {
            value: Some(MyInput(1))
        }
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
        .value = Some(MyInput(2));

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
        &ActionState {
            value: Some(MyInput(2))
        }
    );

    // Advance to client tick to verify server applies the input
    stepper.frame_step((client_tick.0 - server_tick.0) as usize);

    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<ActionState<MyInput>>(server_entity)
            .unwrap(),
        &ActionState {
            value: Some(MyInput(2))
        }
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
        .value = Some(MyInput(3));

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
        &ActionState {
            value: Some(MyInput(3))
        }
    );

    // Advance to client tick to verify server applies the input
    stepper.frame_step((client_tick.0 - server_tick.0) as usize);

    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<ActionState<MyInput>>(server_entity)
            .unwrap(),
        &ActionState {
            value: Some(MyInput(3))
        }
    );
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
