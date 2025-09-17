use crate::protocol::*;
use crate::stepper::ClientServerStepper;
use bevy::ecs::entity::UniqueEntityArray;
use bevy::prelude::*;
use core::fmt::Debug;
use lightyear::prelude::*;
use lightyear_connection::client::PeerMetadata;
use lightyear_messages::multi::MultiMessageSender;
use test_log::test;
use tracing::trace;

#[derive(Resource)]
struct Buffer<M>(Vec<(Entity, M)>);

impl<M> Default for Buffer<M> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

/// System to check that we received the message
fn count_messages_observer<M: Message + Debug>(
    mut receiver: Query<(Entity, &mut MessageReceiver<M>)>,
    mut buffer: ResMut<Buffer<M>>,
) {
    receiver.iter_mut().for_each(|(entity, mut receiver)| {
        receiver.receive().for_each(|m| buffer.0.push((entity, m)));
    })
}

#[test]
fn test_send_messages() {
    let mut stepper = ClientServerStepper::host_server();
    let host_client = stepper.host_client_entity.unwrap();
    let client_of_0 = stepper.client_of_entities[0];
    stepper.server_app.init_resource::<Buffer<StringMessage>>();
    stepper
        .server_app
        .add_systems(Update, count_messages_observer::<StringMessage>);
    stepper
        .client_app()
        .init_resource::<Buffer<StringMessage>>();
    stepper
        .client_app()
        .add_systems(Update, count_messages_observer::<StringMessage>);

    info!("Sending message from host-client {host_client} to server");
    let send_message = StringMessage("Hello".to_string());
    stepper
        .host_client_mut()
        .get_mut::<MessageSender<StringMessage>>()
        .unwrap()
        .send::<Channel1>(send_message.clone());
    stepper.frame_step(2);

    let mut received_messages = stepper
        .server_app
        .world_mut()
        .resource_mut::<Buffer<StringMessage>>();
    assert_eq!(
        &received_messages.0,
        &vec![(stepper.host_client_entity.unwrap(), send_message)]
    );
    received_messages.0.clear();

    info!("Sending message from server to clients (including host-client {host_client})");
    let send_message = StringMessage("World".to_string());
    let send_message_clone = send_message.clone();
    let system_id = stepper
        .server_app
        .register_system(move |mut sender: MultiMessageSender| {
            sender
                .send::<_, Channel1>(&send_message_clone, unsafe {
                    UniqueEntityArray::from_array_unchecked([client_of_0, host_client])
                })
                .ok();
        });
    stepper.server_app.world_mut().run_system(system_id);
    stepper.frame_step(2);

    let mut received_messages = stepper.client_apps[0]
        .world_mut()
        .resource_mut::<Buffer<StringMessage>>();
    assert_eq!(
        &received_messages.0,
        &vec![(stepper.client_entities[0], send_message.clone())]
    );
    received_messages.0.clear();
    let mut received_messages = stepper
        .server_app
        .world_mut()
        .resource_mut::<Buffer<StringMessage>>();
    assert_eq!(&received_messages.0, &vec![(host_client, send_message)]);
    received_messages.0.clear();
}

/// Use ServerMultiMessageSender to send a message from the server to clients, including host-client
#[test]
fn test_send_message_server_to_host_client() {
    let mut stepper = ClientServerStepper::host_server();
    let host_client = stepper.host_client_entity.unwrap();
    let client_of_0 = stepper.client_of_entities[0];
    stepper.server_app.init_resource::<Buffer<StringMessage>>();
    stepper
        .server_app
        .add_systems(Update, count_messages_observer::<StringMessage>);
    stepper
        .client_app()
        .init_resource::<Buffer<StringMessage>>();
    stepper
        .client_app()
        .add_systems(Update, count_messages_observer::<StringMessage>);
    info!(
        "Sending message from server to clients (including host-client {host_client}) via NetworkTarget"
    );
    let send_message = StringMessage("World".to_string());
    let send_message_clone = send_message.clone();
    stepper.server_app.add_systems(
        Update,
        move |mut sender: ServerMultiMessageSender, server: Single<&Server>| {
            sender
                .send::<_, Channel1>(
                    &send_message_clone,
                    server.into_inner(),
                    &NetworkTarget::All,
                )
                .ok();
        },
    );
    stepper.frame_step_server_first(1);
    let mut received_messages = stepper.client_apps[0]
        .world_mut()
        .resource_mut::<Buffer<StringMessage>>();
    assert_eq!(
        &received_messages.0,
        &vec![(stepper.client_entities[0], send_message.clone())]
    );
    received_messages.0.clear();

    // the message is sent by the Server in Update; and the host-client receives it in PreUpdate, so we need
    // to run one more frame
    stepper.frame_step(1);
    let mut received_messages = stepper
        .server_app
        .world_mut()
        .resource_mut::<Buffer<StringMessage>>();
    assert_eq!(&received_messages.0, &vec![(host_client, send_message)]);
    received_messages.0.clear();
}

#[derive(Resource)]
struct TriggerBuffer<M>(Vec<(Entity, M, Entity)>);

impl<M> Default for TriggerBuffer<M> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

/// System to check that we received the message on the server
fn count_triggers_observer<M: Event + Debug + Clone>(
    trigger: On<RemoteEvent<M>>,
    peer_metadata: Res<PeerMetadata>,
    mut buffer: ResMut<TriggerBuffer<M>>,
) {
    info!("Received trigger: {:?}", trigger);
    // Get the entity that is 'receiving' the trigger
    let remote = *peer_metadata.mapping.get(&trigger.from).unwrap();
    buffer
        .0
        .push((remote, trigger.trigger.clone(), trigger.entity));
}

#[test]
fn test_send_triggers() {
    let mut stepper = ClientServerStepper::host_server();
    stepper
        .server_app
        .add_observer(count_triggers_observer::<StringTrigger>);
    stepper
        .server_app
        .init_resource::<TriggerBuffer<StringTrigger>>();

    trace!("Sending trigger from host-client to server");
    let send_trigger = StringTrigger("Hello".to_string());
    stepper
        .host_client_mut()
        .get_mut::<EventSender<StringTrigger>>()
        .unwrap()
        .trigger::<Channel1>(send_trigger.clone());
    stepper.frame_step(2);

    assert_eq!(
        &stepper
            .server_app
            .world()
            .resource::<TriggerBuffer<StringTrigger>>()
            .0,
        &vec![(
            stepper.host_client_entity.unwrap(),
            send_trigger,
            Entity::PLACEHOLDER
        )]
    );

    trace!("Sending trigger from server to clients (including host-client)");
    // TODO: add test for server to clients trigger
}
