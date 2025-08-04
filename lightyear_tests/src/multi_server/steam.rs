//! Tests using the Steam networking layer with Lightyear.
#![allow(unused_imports)]

use crate::stepper::{ClientServerStepper, SERVER_PORT, STEAM_APP_ID};
use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use lightyear::prelude::server::{ListenTarget, SteamServerIo};
use lightyear::prelude::{SessionConfig, SteamAppExt};
use lightyear_connection::network_target::NetworkTarget;
use lightyear_messages::MessageManager;
use lightyear_replication::prelude::Replicate;
use tracing::info;

struct StepperPointer(*mut ClientServerStepper);

fn add_steam_server_io(stepper: &mut ClientServerStepper) {
    stepper.server_app.add_steam_resources(STEAM_APP_ID);
    let server_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, SERVER_PORT));
    stepper.server_mut().insert(SteamServerIo {
        target: ListenTarget::Addr(server_addr),
        config: SessionConfig::default(),
    });
}

/// Test that it is possible to create a Server entity with both the SteamServerIO and the NetcodeServerIO components.
#[cfg(not(feature = "test_utils"))]
#[test]
fn test_steam_server_with_netcode_server() {
    let mut stepper = ClientServerStepper::default_no_init();
    // start the server first and make sure the SteamServer is Started
    info!("Starting server app");
    add_steam_server_io(&mut stepper);
    stepper.init();
    // wait to make sure the server is started
    stepper.frame_step(10);

    info!("Server app started, now adding a steam client");
    // then add a steam client (client 0)
    stepper.new_steam_client();
    // add a non-steam client (client 1)
    stepper.new_client();
    stepper.init();

    // check that io is working
    let server_entity = stepper
        .server_app
        .world_mut()
        .spawn(Replicate::to_clients(NetworkTarget::All))
        .id();
    stepper.frame_step_server_first(1);
    stepper
        .client(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .unwrap();
    stepper
        .client(1)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(server_entity)
        .unwrap();
}
