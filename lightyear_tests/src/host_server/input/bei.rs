use crate::protocol::{BEIAction1, BEIContext};
use crate::stepper::*;
use bevy::prelude::*;
use bevy_enhanced_input::action::{Action, ActionMock, ActionState, MockSpan};
use bevy_enhanced_input::prelude::{ActionOf, ActionValue, Actions};
use lightyear::input::bei::input_message::BEIBuffer;
use lightyear::prelude::input::bei::InputMarker;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::{PredictionTarget, Replicate, ReplicateLike};

/// Check that the host-server still rebroadcasts inputs from non-host clients to each other.
#[test]
fn test_rebroadcast() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::from_link_types(
        vec![ClientType::Host, ClientType::Netcode, ClientType::Netcode],
        ServerType::Netcode,
    ));

    // Entity controlled by client 1, with inputs rebroadcast to all clients
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
    let client0_entity = stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity not replicated to client 1");
    let client1_entity = stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .expect("entity not replicated to client 2");

    // Spawn action on client 0
    let action0 = stepper.client_apps[0]
        .world_mut()
        .spawn((
            ActionOf::<BEIContext>::new(client1_entity),
            Action::<BEIAction1>::default(),
            ActionMock::new(
                ActionState::Fired,
                ActionValue::Bool(true),
                MockSpan::Manual,
            ),
        ))
        .id();
    stepper.frame_step(4);

    // Check that client 1 received the rebroadcasted action
    let action1 = stepper.client_apps[1]
        .world()
        .get::<Actions<BEIContext>>(client1_entity)
        .unwrap()
        .collection()[0];
    assert!(
        stepper.client_apps[1]
            .world()
            .entity(action1)
            .get::<BEIBuffer<BEIContext>>()
            .is_some()
    );

    // Check that the host-server did not add an InputMarker on the action entity
    let action_host = stepper
        .server_app
        .world()
        .get::<Actions<BEIContext>>(server_entity)
        .unwrap()
        .collection()[0];
    assert!(
        stepper
            .server_app
            .world()
            .get::<InputMarker<BEIContext>>(action_host)
            .is_none()
    );
    assert!(
        stepper
            .server_app
            .world()
            .get::<Replicate>(action_host)
            .is_none()
    );
    assert!(
        stepper
            .server_app
            .world()
            .get::<ReplicateLike>(action_host)
            .is_some()
    );

    // Check that
}
