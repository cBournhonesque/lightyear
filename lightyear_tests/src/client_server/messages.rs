use crate::protocol::*;
use crate::stepper::ClientServerStepper;
use bevy::prelude::*;
use core::fmt::Debug;
use lightyear::prelude::*;
use tracing::trace;


#[test_log::test]
fn test_send_messages() {
    let mut stepper = ClientServerStepper::single();

    trace!("Sending message from client to server");
    let send_message = StringMessage("Hello".to_string());
    stepper.client_mut(0).get_mut::<MessageSender<StringMessage>>().unwrap().send::<Channel1>(send_message.clone());
    stepper.frame_step(1);

    let receive_message = stepper.client_of_mut(0).get_mut::<MessageReceiver<StringMessage>>().unwrap().receive().next().unwrap();
    assert_eq!(receive_message, send_message);

    trace!("Sending message from server to client");
    let send_message = StringMessage("World".to_string());
    stepper.client_mut(0).get_mut::<MessageSender<StringMessage>>().unwrap().send::<Channel1>(send_message.clone());
    stepper.frame_step(2);

    let receive_message = stepper.client_of_mut(0).get_mut::<MessageReceiver<StringMessage>>().unwrap().receive().next().unwrap();
    assert_eq!(receive_message, send_message);
}

#[derive(Resource, Default)]
struct Counter(usize);

/// System to check that we received the message on the server
fn count_triggers_observer<M: Event + Debug>(
    trigger: Trigger<RemoteTrigger<M>>,
    mut counter: ResMut<Counter>,
) {
    info!("Received trigger: {:?}", trigger);
    counter.0 += 1;
}


// TODO: check trigger with entity map
#[test_log::test]
fn test_send_triggers() {
    let mut stepper = ClientServerStepper::single();
    stepper.server_app.add_observer(count_triggers_observer::<StringTrigger>);
    stepper.server_app.add_observer(count_triggers_observer::<EntityTrigger>);
    stepper.server_app.init_resource::<Counter>();

    trace!("Sending trigger from client to server");
    let send_trigger = StringTrigger("Hello".to_string());
    stepper.client_mut(0).get_mut::<TriggerSender<StringTrigger>>().unwrap().trigger::<Channel1>(send_trigger.clone());
    stepper.frame_step(1);

    assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);
}
