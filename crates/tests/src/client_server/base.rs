use crate::stepper::*;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear_connection::server::{Stop, Stopped};
use test_log::test;

/// Exercises the same stepper and connection stack as the other client/server tests, but with a
/// real local WebTransport session. The OS-selected port makes this safe to run alongside other
/// tests without maintaining a separate WebTransport harness.
#[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
#[test]
fn test_setup_netcode_webtransport_client_server() {
    let mut stepper =
        ClientServerStepper::from_config(StepperConfig::single().with_io(IoType::WebTransport));

    assert_ne!(stepper.server_addr.port(), 0);
    assert_eq!(
        stepper.server().get::<WebTransportLocalAddr>().unwrap().0,
        stepper.server_addr
    );
    let peer = stepper.client(0).get::<WebTransportPeerAddr>().unwrap().0;
    assert_eq!(peer.ip().to_canonical(), stepper.server_addr.ip());
    assert_eq!(peer.port(), stepper.server_addr.port());
    assert!(stepper.client(0).contains::<Connected>());
    assert!(stepper.client_of(0).contains::<Connected>());
    assert!(input_timeline_is_synced(stepper.client_app().world()));
    assert!(interpolation_timeline_is_synced(
        stepper.client_app().world()
    ));
}

#[test]
fn test_stop_netcode_server_unlinks_transport() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let server = stepper.server_entity;

    stepper
        .server_app
        .world_mut()
        .trigger(Stop { entity: server });
    stepper.frame_step_server_first(10);

    let server = stepper.server();
    assert!(server.contains::<Stopped>());
    assert!(server.contains::<Unlinked>());
    assert!(!server.contains::<Linked>());
}

/// Check that the client/server setup is correct when the connection type is Raw instead of Netcode
#[test]
fn test_setup_raw_client_server() {
    let stepper = ClientServerStepper::from_config(StepperConfig::from_connection_types(
        vec![ClientType::Raw],
        ServerType::Raw,
    ));
    assert!(stepper.client(0).contains::<Connected>());

    assert!(stepper.client_of(0).contains::<Connected>());
}

#[test]
fn test_sender_metadata() {
    let stepper = ClientServerStepper::from_config(StepperConfig::single());
    let client = stepper.client(0).id();
    let client_of = stepper.client_of(0).id();

    assert_eq!(
        stepper
            .client_of(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(client)
            .expect("client is not present in entity map"),
        client_of
    );
    // NOTE: the (client_of -> client) connection pair lives only in the local
    // `MessageManager` map (populated from `SenderMetadata`); replicon's map
    // only tracks replicated entities, so this lookup intentionally does not
    // go through `ServerEntityMap`.
    assert_eq!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(client_of)
            .expect("client_of is not present in entity map"),
        client
    );
}
