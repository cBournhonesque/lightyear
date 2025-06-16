use crate::protocol::StringMessage;
use crate::stepper::ClientServerStepper;
use lightyear::prelude::InterpolationTimeline;
use lightyear_connection::client::Connected;
use lightyear_connection::host::{HostClient, HostServer};
use lightyear_connection::server::Started;
use lightyear_core::id::{LocalId, RemoteId};
use lightyear_core::prelude::LocalTimeline;
use lightyear_messages::prelude::{MessageReceiver, MessageSender, TriggerSender};
use lightyear_messages::MessageManager;
use lightyear_replication::message::SenderMetadata;
use lightyear_replication::prelude::{ReplicationReceiver, ReplicationSender};
use lightyear_sync::prelude::client::RemoteTimeline;
use lightyear_sync::prelude::{InputTimeline, IsSynced};
use lightyear_transport::prelude::Transport;
use test_log::test;

/// Check that the client/server setup is correct:
/// - the various components we expect are present
#[test]
fn test_setup_host_server() {
    let stepper = ClientServerStepper::host_server();

    // Check that the various components we expect are present
    assert!(stepper.host_client().contains::<HostClient>());
    // Input/Remote timeline are required by Client.
    // TODO: update InputTimeline to match LocalTimeline and to not sync
    assert!(stepper.host_client().contains::<InputTimeline>());
    assert!(stepper.host_client().contains::<IsSynced<InputTimeline>>());
    assert!(stepper.host_client().contains::<RemoteTimeline>());
    // TODO: update Interpolation to be disabled for host-clients!
    assert!(stepper.host_client().contains::<InterpolationTimeline>());
    assert!(stepper.host_client().contains::<IsSynced<InterpolationTimeline>>());
    assert!(stepper.host_client().contains::<LocalTimeline>());
    assert!(stepper.host_client().contains::<Transport>());
    assert!(stepper.host_client().contains::<MessageManager>());
    assert!(stepper.host_client().contains::<MessageSender<StringMessage>>());
    assert!(
        stepper
            .host_client()
            .contains::<MessageReceiver<StringMessage>>()
    );
    assert!(
        stepper
            .host_client()
            .contains::<TriggerSender<SenderMetadata>>()
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