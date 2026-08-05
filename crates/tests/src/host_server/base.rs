use crate::protocol::StringMessage;
use crate::stepper::*;
use lightyear::prelude::InterpolationTimeline;
use lightyear_connection::client::{Connect, Connected, Connecting};
use lightyear_connection::host::{HostClient, HostServer};
use lightyear_connection::server::{Start, Started};
use lightyear_core::id::{LocalId, RemoteId};
use lightyear_link::Linked;
use lightyear_messages::MessageManager;
use lightyear_messages::prelude::{EventSender, MessageReceiver, MessageSender};
use lightyear_replication::prelude::{ReplicationReceiver, ReplicationSender, SenderMetadata};
use lightyear_sync::prelude::client::RemoteTimeline;
use lightyear_sync::prelude::{InputTimeline, IsSynced};
use lightyear_transport::prelude::Transport;
use test_log::test;

/// Check that the client/server setup is correct:
/// - the various components we expect are present
#[test]
fn test_setup_host_server() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::host_server());
    stepper.frame_step(1);

    // Check that the various components we expect are present
    assert!(stepper.host_client().contains::<HostClient>());
    // Input/Remote timeline are required by Client.
    // TODO: update InputTimeline to match LocalTimeline and to not sync
    assert!(stepper.host_client().contains::<InputTimeline>());
    assert!(stepper.host_client().contains::<IsSynced<InputTimeline>>());
    assert!(stepper.host_client().contains::<RemoteTimeline>());
    // TODO: update Interpolation to be disabled for host-clients!
    assert!(stepper.host_client().contains::<InterpolationTimeline>());
    assert!(
        stepper
            .host_client()
            .contains::<IsSynced<InterpolationTimeline>>()
    );
    assert!(stepper.host_client().contains::<Transport>());
    assert!(stepper.host_client().contains::<MessageManager>());
    assert!(
        stepper
            .host_client()
            .contains::<MessageSender<StringMessage>>()
    );
    // Message receivers are created lazily when the first payload arrives.
    assert!(
        !stepper
            .host_client()
            .contains::<MessageReceiver<StringMessage>>()
    );
    assert!(
        stepper
            .host_client()
            .contains::<EventSender<SenderMetadata>>()
    );
    // no need to replicate between the host-client and the server
    assert!(!stepper.host_client().contains::<ReplicationSender>());
    assert!(!stepper.host_client().contains::<ReplicationReceiver>());
    assert!(stepper.host_client().contains::<Connected>());
    assert!(stepper.host_client().contains::<LocalId>());
    assert!(stepper.host_client().contains::<RemoteId>());

    assert!(stepper.server().contains::<HostServer>());
    assert!(stepper.server().contains::<Started>());
}

/// A raw server only becomes `Started` after its IO becomes `Linked`. If the host-client requests
/// a connection first, that request should remain pending and be retried when the server starts.
#[test]
fn test_raw_host_connects_after_server_linked() {
    let mut config = StepperConfig::from_connection_types(vec![ClientType::Host], ServerType::Raw);
    config.init = false;
    let mut stepper = ClientServerStepper::from_config(config);
    let server = stepper.server_entity;
    let host_client = stepper.host_client_entity.unwrap();

    stepper
        .server_app
        .world_mut()
        .trigger(Start { entity: server });
    stepper.server_app.world_mut().flush();
    stepper.server_app.world_mut().trigger(Connect {
        entity: host_client,
    });
    stepper.server_app.world_mut().flush();

    assert!(!stepper.server().contains::<Started>());
    assert!(stepper.host_client().contains::<Connecting>());

    // Simulate an asynchronous raw server IO completing its LinkStart request.
    stepper.server_mut().insert(Linked);
    stepper.server_app.world_mut().flush();

    assert!(stepper.server().contains::<Started>());
    assert!(stepper.server().contains::<HostServer>());
    assert!(stepper.host_client().contains::<Connected>());
    assert!(stepper.host_client().contains::<HostClient>());
    assert!(!stepper.host_client().contains::<Connecting>());
}
