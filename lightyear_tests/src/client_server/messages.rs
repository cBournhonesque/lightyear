use crate::protocol::*;
use crate::stepper::ClientServerStepper;
use bevy::prelude::*;
use core::fmt::Debug;
use lightyear::prelude::*;
use test_log::test;
use tracing::trace;

#[derive(Resource)]
struct Buffer<M>(Vec<M>);

impl<M> Default for Buffer<M> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

/// System to check that we received the message on the server
fn count_messages_observer<M: Message + Debug>(
    mut receiver: Single<&mut MessageReceiver<M>>,
    mut buffer: ResMut<Buffer<M>>,
) {
    receiver.receive().for_each(|m| buffer.0.push(m));
}

#[test]
fn test_send_messages() {
    let mut stepper = ClientServerStepper::single();
    stepper.server_app.init_resource::<Buffer<StringMessage>>();
    stepper.server_app.add_systems(Update, count_messages_observer::<StringMessage>);
    stepper.client_app.init_resource::<Buffer<StringMessage>>();
    stepper.client_app.add_systems(Update, count_messages_observer::<StringMessage>);

    info!("Sending message from client to server");
    let send_message = StringMessage("Hello".to_string());
    stepper.client_mut(0).get_mut::<MessageSender<StringMessage>>().unwrap().send::<Channel1>(send_message.clone());
    stepper.frame_step(1);

    let received_messages = stepper.server_app.world().resource::<Buffer<StringMessage>>();
    assert_eq!(&received_messages.0, &vec![send_message]);

    info!("Sending message from server to client");
    let send_message = StringMessage("World".to_string());
    stepper.client_of_mut(0).get_mut::<MessageSender<StringMessage>>().unwrap().send::<Channel1>(send_message.clone());
    stepper.frame_step(2);

    let received_messages = stepper.client_app.world().resource::<Buffer<StringMessage>>();
    assert_eq!(&received_messages.0, &vec![send_message]);
}



/// System to check that we received the message on the server
fn count_triggers_observer<M: Event + Debug + Clone>(
    trigger: Trigger<RemoteTrigger<M>>,
    mut buffer: ResMut<Buffer<M>>,
) {
    info!("Received trigger: {:?}", trigger);
    buffer.0.push(trigger.trigger.clone());
}


// TODO: check trigger with entity map
#[test]
fn test_send_triggers() {
    let mut stepper = ClientServerStepper::single();
    stepper.server_app.add_observer(count_triggers_observer::<StringTrigger>);
    stepper.server_app.init_resource::<Buffer<StringTrigger>>();

    trace!("Sending trigger from client to server");
    let send_trigger = StringTrigger("Hello".to_string());
    stepper.client_mut(0).get_mut::<TriggerSender<StringTrigger>>().unwrap().trigger::<Channel1>(send_trigger.clone());
    stepper.frame_step(1);

    assert_eq!(&stepper.server_app.world().resource::<Buffer<StringTrigger>>().0, &vec![send_trigger]);
}
