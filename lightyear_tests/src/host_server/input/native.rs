use crate::protocol::NativeInput as MyInput;
use crate::stepper::ClientServerStepper;
use lightyear::input::native::prelude::{InputMarker, NativeBuffer};
use lightyear::prelude::input::native::ActionState;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::{PredictionTarget, Replicate};
use lightyear_sync::prelude::InputTimeline;
use lightyear_sync::prelude::client::IsSynced;
use test_log::test;
use tracing::info;

/// Test a remote client's replicated entity sending inputs to the server
///
/// /// (we run this test to ensure that things still work in the HostServer case)
#[test]
fn test_remote_client_replicated_input() {
    let mut stepper = ClientServerStepper::host_server();

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
            .get::<NativeBuffer<MyInput>>(server_entity)
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
///
/// (we run this test to ensure that things still work in the HostServer case)
#[test]
fn test_remote_client_predicted_input() {
    let mut stepper = ClientServerStepper::host_server();

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

    let client_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity was not replicated to client");
    info!(?client_entity, "client entities");

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
        .0 = MyInput(2);

    stepper.frame_step(1);
    let server_tick = stepper.server_tick();
    let client_tick = stepper.client_tick(0);

    // ASSERT
    assert_eq!(
        stepper
            .server_app
            .world()
            .get::<NativeBuffer<MyInput>>(server_entity)
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

/// Test the host client's inputs are replicated to the remote client
/// i.e. chost-client inputs are broadcasted to other clients for prediction
#[test]
fn test_host_client_inputers_replicated_to_remote_client() {
    let mut stepper = ClientServerStepper::host_server();

    stepper
        .server_app
        .world_mut()
        .query::<&IsSynced<InputTimeline>>()
        .single(stepper.server_app.world())
        .unwrap();

    // SETUP
    // entity controlled by the host client
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::All),
            InputMarker::<MyInput>::default(),
        ))
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
        .server_app
        .world_mut()
        .get_mut::<ActionState<MyInput>>(server_entity)
        .unwrap()
        .0 = MyInput(1);

    stepper.frame_step(1);
    let server_tick = stepper.server_tick();
    info!("Set input at server tick {server_tick:?}");
    // by this point, the host-client has sent-local an InputMessage to the Server

    // second frame step so that the client receives the server's rebroadcasted input
    // frame 1: server receives the InputMessage and rebroadcasts
    // frame 2: client receives the InputMessage and adds to InputBuffer
    stepper.frame_step(2);

    // Client send an InputMessage to the server, who then adds an InputBuffer.
    assert_eq!(
        stepper
            .client_app()
            .world()
            .get::<NativeBuffer<MyInput>>(client_entity)
            .unwrap()
            .get(server_tick)
            .unwrap(),
        &ActionState(MyInput(1))
    );
}
