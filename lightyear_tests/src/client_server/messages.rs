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

/// System to check that we received the message on the server
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
    let mut stepper = ClientServerStepper::single();
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

    info!("Sending message from client to server");
    let send_message = StringMessage("Hello".to_string());
    stepper
        .client_mut(0)
        .get_mut::<MessageSender<StringMessage>>()
        .unwrap()
        .send::<Channel1>(send_message.clone());
    stepper.frame_step(1);

    let received_messages = stepper
        .server_app
        .world()
        .resource::<Buffer<StringMessage>>();
    assert_eq!(
        &received_messages.0,
        &vec![(stepper.client_of_entities[0], send_message)]
    );

    info!("Sending message from server to client");
    let send_message = StringMessage("World".to_string());
    stepper
        .client_of_mut(0)
        .get_mut::<MessageSender<StringMessage>>()
        .unwrap()
        .send::<Channel1>(send_message.clone());
    stepper.frame_step(2);

    let received_messages = stepper.client_apps[0]
        .world()
        .resource::<Buffer<StringMessage>>();
    assert_eq!(
        &received_messages.0,
        &vec![(stepper.client_entities[0], send_message)]
    );
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
    let mut stepper = ClientServerStepper::single();
    stepper
        .server_app
        .add_observer(count_triggers_observer::<StringTrigger>);
    stepper
        .server_app
        .init_resource::<TriggerBuffer<StringTrigger>>();

    trace!("Sending trigger from client to server");
    let send_trigger = StringTrigger("Hello".to_string());
    stepper
        .client_mut(0)
        .get_mut::<EventSender<StringTrigger>>()
        .unwrap()
        .trigger::<Channel1>(send_trigger.clone());
    stepper.frame_step(1);

    assert_eq!(
        &stepper
            .server_app
            .world()
            .resource::<TriggerBuffer<StringTrigger>>()
            .0,
        &vec![(
            stepper.client_of_entities[0],
            send_trigger,
            Entity::PLACEHOLDER
        )]
    );
}

#[test]
fn test_send_triggers_map_entities() {
    let mut stepper = ClientServerStepper::single();
    let client_entity = stepper
        .client_app()
        .world_mut()
        .spawn(Replicate::to_server())
        .id();
    stepper.frame_step(1);
    let server_entity = stepper
        .client_of(0)
        .get::<MessageManager>()
        .unwrap()
        .entity_mapper
        .get_local(client_entity)
        .expect("entity is not present in entity map");

    stepper
        .server_app
        .add_observer(count_triggers_observer::<EntityTrigger>);
    stepper
        .server_app
        .init_resource::<TriggerBuffer<EntityTrigger>>();

    trace!("Sending trigger from client to server");
    let send_trigger = EntityTrigger(client_entity);
    stepper
        .client_mut(0)
        .get_mut::<EventSender<EntityTrigger>>()
        .unwrap()
        .trigger_targets::<Channel1>(send_trigger, core::iter::once(client_entity));
    stepper.frame_step(1);

    assert_eq!(
        &stepper
            .server_app
            .world()
            .resource::<TriggerBuffer<EntityTrigger>>()
            .0,
        &vec![(
            stepper.client_of_entities[0],
            EntityTrigger(server_entity),
            server_entity
        )]
    );
}

/// Test sending a message to multiple clients
#[test]
fn test_send_multi_messages() {
    let mut stepper = ClientServerStepper::with_clients(2);

    stepper.client_apps[0].init_resource::<Buffer<StringMessage>>();
    stepper.client_apps[0].add_systems(Update, count_messages_observer::<StringMessage>);

    stepper.client_apps[1].init_resource::<Buffer<StringMessage>>();
    stepper.client_apps[1].add_systems(Update, count_messages_observer::<StringMessage>);

    info!("Sending messages from server to client");
    let send_message = StringMessage("World".to_string());
    let message = send_message.clone();
    let client_of_0 = stepper.client_of_entities[0];
    let client_of_1 = stepper.client_of_entities[1];

    let system_id = stepper
        .server_app
        .register_system(move |mut sender: MultiMessageSender| {
            sender
                .send::<_, Channel1>(&message, unsafe {
                    UniqueEntityArray::from_array_unchecked([client_of_0, client_of_1])
                })
                .ok();
        });
    stepper.server_app.world_mut().run_system(system_id);
    stepper.frame_step(2);

    let received_messages = stepper.client_apps[0]
        .world()
        .resource::<Buffer<StringMessage>>();
    assert!(
        &received_messages
            .0
            .contains(&(stepper.client_entities[0], send_message.clone()))
    );
    let received_messages = stepper.client_apps[1]
        .world()
        .resource::<Buffer<StringMessage>>();
    assert!(
        &received_messages
            .0
            .contains(&(stepper.client_entities[1], send_message.clone()))
    );
}

/// Test sending a message to multiple clients with NetworkTarget
#[test]
fn test_send_multi_messages_with_target() {
    let mut stepper = ClientServerStepper::with_clients(2);

    stepper.client_apps[0].init_resource::<Buffer<StringMessage>>();
    stepper.client_apps[0].add_systems(Update, count_messages_observer::<StringMessage>);
    stepper.client_apps[1].init_resource::<Buffer<StringMessage>>();
    stepper.client_apps[1].add_systems(Update, count_messages_observer::<StringMessage>);

    info!("Sending messages from server to client");
    let send_message = StringMessage("World".to_string());
    let message = send_message.clone();
    let system_id = stepper.server_app.register_system(
        move |mut sender: ServerMultiMessageSender, server: Single<&Server>| {
            sender
                .send::<_, Channel1>(&message, server.into_inner(), &NetworkTarget::All)
                .ok();
        },
    );
    stepper.server_app.world_mut().run_system(system_id);
    stepper.frame_step(2);

    let received_messages = stepper.client_apps[0]
        .world()
        .resource::<Buffer<StringMessage>>();
    assert!(
        &received_messages
            .0
            .contains(&(stepper.client_entities[0], send_message.clone()))
    );
    let received_messages = stepper.client_apps[1]
        .world()
        .resource::<Buffer<StringMessage>>();
    assert!(
        &received_messages
            .0
            .contains(&(stepper.client_entities[1], send_message.clone()))
    );
}
