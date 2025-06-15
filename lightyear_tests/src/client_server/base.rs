use crate::protocol::StringMessage;
use crate::stepper::ClientServerStepper;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear_connection::server::Started;
use lightyear_crossbeam::CrossbeamIo;
use test_log::test;

/// Check that the client/server setup is correct:
/// - the various components we expect are present
#[test]
fn test_setup_client_server() {
    let stepper = ClientServerStepper::single();

    // Check that the various components we expect are present
    assert!(stepper.client(0).contains::<PingManager>());
    assert!(stepper.client(0).contains::<InputTimeline>());
    assert!(stepper.client(0).contains::<RemoteTimeline>());
    assert!(stepper.client(0).contains::<LocalTimeline>());
    assert!(stepper.client(0).contains::<InterpolationTimeline>());
    assert!(stepper.client(0).contains::<Transport>());
    assert!(stepper.client(0).contains::<MessageManager>());
    assert!(stepper.client(0).contains::<MessageSender<StringMessage>>());
    assert!(
        stepper
            .client(0)
            .contains::<MessageReceiver<StringMessage>>()
    );
    assert!(
        stepper
            .client(0)
            .contains::<TriggerSender<SenderMetadata>>()
    );
    assert!(
        stepper
            .client(0)
            .contains::<TriggerSender<AuthorityTransferRequest>>()
    );
    assert!(
        stepper
            .client(0)
            .contains::<TriggerSender<AuthorityTransferResponse>>()
    );
    assert!(stepper.client(0).contains::<ReplicationSender>());
    assert!(stepper.client(0).contains::<ReplicationReceiver>());
    assert!(stepper.client(0).contains::<CrossbeamIo>());
    assert!(stepper.client(0).contains::<Connected>());
    assert!(stepper.client(0).contains::<LocalAddr>());
    assert!(stepper.client(0).contains::<PeerAddr>());
    assert!(stepper.client(0).contains::<LocalId>());
    assert!(stepper.client(0).contains::<RemoteId>());

    assert!(stepper.server().contains::<LocalTimeline>());
    assert!(stepper.server().contains::<Started>());

    assert!(stepper.client_of(0).contains::<LocalTimeline>());
    assert!(stepper.client_of(0).contains::<Transport>());
    assert!(stepper.client_of(0).contains::<MessageManager>());
    assert!(
        stepper
            .client_of(0)
            .contains::<MessageSender<StringMessage>>()
    );
    assert!(
        stepper
            .client_of(0)
            .contains::<MessageReceiver<StringMessage>>()
    );
    assert!(
        stepper
            .client_of(0)
            .contains::<TriggerSender<SenderMetadata>>()
    );
    assert!(
        stepper
            .client_of(0)
            .contains::<TriggerSender<AuthorityTransferRequest>>()
    );
    assert!(
        stepper
            .client_of(0)
            .contains::<TriggerSender<AuthorityTransferResponse>>()
    );
    assert!(stepper.client_of(0).contains::<CrossbeamIo>());
    assert!(stepper.client_of(0).contains::<Connected>());
    assert!(stepper.client_of(0).contains::<PeerAddr>());
    assert!(stepper.client_of(0).contains::<LocalId>());
    assert!(stepper.client_of(0).contains::<RemoteId>());
}



