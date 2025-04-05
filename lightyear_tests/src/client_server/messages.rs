use crate::protocol::*;
use crate::stepper::ClientServerStepper;
use lightyear_new::prelude::*;
use tracing::trace;

/// Check that the client/server setup is correct:
/// - the various components we expect are present
#[test_log::test]
fn test_send_messages() {
    let mut stepper = ClientServerStepper::default();

    trace!("Sending message from client to server");
    let send_message = StringMessage("Hello".to_string());
    stepper.client_mut().get_mut::<MessageSender<StringMessage>>().unwrap().send::<Channel1>(send_message.clone());
    stepper.frame_step();

    let receive_message = stepper.client_1_mut().get_mut::<MessageReceiver<StringMessage>>().unwrap().receive().next().unwrap();
    assert_eq!(receive_message, send_message);

    trace!("Sending message from server to client");
    let send_message = StringMessage("World".to_string());
    stepper.client_1_mut().get_mut::<MessageSender<StringMessage>>().unwrap().send::<Channel1>(send_message.clone());
    stepper.frame_step();
    stepper.frame_step();

    let receive_message = stepper.client_mut().get_mut::<MessageReceiver<StringMessage>>().unwrap().receive().next().unwrap();
    assert_eq!(receive_message, send_message);
}
