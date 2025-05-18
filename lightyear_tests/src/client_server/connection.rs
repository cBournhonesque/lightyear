//! Check various replication scenarios between 2 peers only

use crate::stepper::ClientServerStepper;
use bevy::prelude::{Entity, With};
use lightyear_connection::client_of::ClientOf;
use test_log::test;

#[test]
fn test_disconnection() {
    let mut stepper = ClientServerStepper::single();

    stepper.disconnect_client();

    // check that the client is not present in the server world
    assert!(
        stepper
            .server_app
            .world_mut()
            .query_filtered::<Entity, With<ClientOf>>()
            .single(stepper.server_app.world())
            .is_err()
    );
}
