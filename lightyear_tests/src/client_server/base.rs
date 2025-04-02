use crate::protocol::StringMessage;
use crate::stepper::ClientServerStepper;
use lightyear_connection::server::Started;
use lightyear_crossbeam::CrossbeamIo;
use lightyear_new::prelude::client::*;
use lightyear_new::prelude::*;

/// Check that the client/server setup is correct:
/// - the various components we expect are present
#[test_log::test]
fn test_setup_client_server() {
    let stepper = ClientServerStepper::default();

    // Check that the various components we expect are present
    assert!(stepper.client().contains::<PingManager>());
    assert!(stepper.client().contains::<InputTimeline>());
    assert!(stepper.client().contains::<RemoteTimeline>());
    assert!(stepper.client().contains::<InterpolationTimeline>());
    assert!(stepper.client().contains::<Transport>());
    assert!(stepper.client().contains::<MessageManager>());
    assert!(stepper.client().contains::<MessageSender<StringMessage>>());
    assert!(stepper.client().contains::<MessageReceiver<StringMessage>>());
    assert!(stepper.client().contains::<CrossbeamIo>());
    assert!(stepper.client().contains::<Connected>());

    assert!(stepper.server().contains::<Timeline<server::Local>>());
    assert!(stepper.server().contains::<Started>());

    assert!(stepper.client_1().contains::<Timeline<server::Local>>());
    assert!(stepper.client_1().contains::<Transport>());
    assert!(stepper.client_1().contains::<MessageManager>());
    assert!(stepper.client_1().contains::<MessageSender<StringMessage>>());
    assert!(stepper.client_1().contains::<MessageReceiver<StringMessage>>());
    assert!(stepper.client_1().contains::<CrossbeamIo>());
    assert!(stepper.client_1().contains::<Connected>());
}