//! Defines the [`ClientMessage`] enum used to send messages from the client to the server
use bevy::ecs::system::{FilteredResourcesMutParamBuilder, ParamBuilder};
use bevy::prelude::*;
use byteorder::WriteBytesExt;
use bytes::Bytes;
use tracing::error;

use crate::client::connection::ConnectionManager;
use crate::prelude::{client::is_connected, ClientId, MainSet};
use crate::protocol::message::{MessageRegistry, MessageType};
use crate::serialize::reader::Reader;
use crate::serialize::{SerializationError, ToBytes};
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{ClientMarker, InternalMainSet};

pub struct ClientMessagePlugin;

impl Plugin for ClientMessagePlugin {
    fn build(&self, app: &mut App) {}

    /// Add the system after all messages have been added to the MessageRegistry
    fn cleanup(&self, app: &mut App) {
        // temporarily remove message_registry from the app to enable split borrows
        let message_registry = app
            .world_mut()
            .remove_resource::<MessageRegistry>()
            .unwrap();

        // Use FilteredResourceMut SystemParam to register the access dynamically to the
        // Messages in the MessageRegistry
        let send_messages = (
            FilteredResourcesMutParamBuilder::new(|builder| {
                message_registry
                    .send_messages
                    .iter()
                    .filter(|metadata| {
                        metadata.message_type == MessageType::Normal
                            || metadata.message_type == MessageType::Event
                    })
                    .for_each(|metadata| {
                        builder.add_write_by_id(metadata.component_id);
                    });
            }),
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
        ).build_state(app.world_mut())
            .build_system(send_messages);

        let read_messages = (
            FilteredResourcesMutParamBuilder::new(|builder| {
                message_registry
                    .message_receive_map
                    .values()
                    .filter(|metadata| {
                        metadata.message_type == MessageType::Normal
                            || metadata.message_type == MessageType::Event
                    })
                    .for_each(|metadata| {
                        builder.add_write_by_id(metadata.component_id);
                    });
            }),
            ParamBuilder,
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(read_messages);
        app.add_systems(
            PreUpdate,
            read_messages
                .in_set(InternalMainSet::<ClientMarker>::ReceiveEvents)
                .run_if(is_connected),
        );
        app.add_systems(
            PostUpdate,
            send_messages.in_set(InternalMainSet::<ClientMarker>::SendEvents)
        );
        app.configure_sets(PostUpdate, InternalMainSet::<ClientMarker>::SendEvents
            .in_set(MainSet::SendEvents)
            .before(InternalMainSet::<ClientMarker>::Send)
        );

        app.insert_resource(message_registry);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClientMessage {
    /// Used if you want to automatically rebroadcast a message to a specific target
    pub(crate) target: NetworkTarget,
    pub(crate) message: Bytes,
}

impl ToBytes for ClientMessage {
    fn len(&self) -> usize {
        self.target.len() + self.message.len()
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        self.target.to_bytes(buffer)?;
        // NOTE: we just write the message bytes directly! We don't provide the length
        buffer.write_all(&self.message)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let target = NetworkTarget::from_bytes(buffer)?;
        // NOTE: this only works if the reader only contains the ClientMessage bytes!
        let remaining = buffer.remaining();
        let message = buffer.split_len(remaining);
        Ok(Self { message, target })
    }
}

/// Read the messages that were buffered as SendMessage<E>
/// and buffer them to be written in the various channels
fn send_messages(
    mut send_events: FilteredResourcesMut,
    commands: Commands,
    message_registry: ResMut<MessageRegistry>,
    mut connection: ResMut<ConnectionManager>,
) {
    let connection = connection.as_mut();
    let _ = message_registry.send_messages(
        &mut send_events,
        &mut connection.message_manager,
        &mut connection.replication_receiver.remote_entity_map.local_to_remote,
        true,
    ).inspect_err(|e| error!("Could not buffer message to send: {:?}", e));
}

/// Read the messages received from the server and handle them:
/// - Messages: send a MessageEvent
/// - Events: send them to EventWriter or trigger them
fn read_messages(
    mut events: FilteredResourcesMut,
    mut commands: Commands,
    message_registry: ResMut<MessageRegistry>,
    mut connection: ResMut<ConnectionManager>,
) {
    // enable split-borrows
    let connection = connection.as_mut();
    connection
        .received_messages
        .drain(..)
        .for_each(|(net_id, message)| {
            let mut reader = Reader::from(message);
            let _ = message_registry
                // we have to re-decode the net id
                .receive_message(
                    net_id,
                    &mut commands,
                    &mut events,
                    // TODO: include the client that rebroadcasted the message?
                    ClientId::Local(0),
                    &mut reader,
                    &mut connection
                        .replication_receiver
                        .remote_entity_map
                        .remote_to_local,
                )
                .inspect_err(|e| error!("Could not deserialize message: {:?}", e));
        });
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialize::writer::Writer;
    use crate::tests::host_server_stepper::HostServerStepper;
    use crate::tests::protocol::{Channel1, StringMessage};
    use bevy::prelude::{EventReader, Resource, Update};

    #[test]
    fn client_message_serde() {
        let data = ClientMessage {
            target: NetworkTarget::None,
            message: Bytes::from_static(b"hello world"),
        };
        let mut writer = Writer::default();
        data.to_bytes(&mut writer).unwrap();
        let bytes = writer.to_bytes();

        let mut reader = Reader::from(bytes);
        let result = ClientMessage::from_bytes(&mut reader).unwrap();
        assert_eq!(data, result);
    }

    #[derive(Resource, Default)]
    struct Counter(usize);

    /// System to check that we received the message on the server
    fn count_messages(
        mut counter: ResMut<Counter>,
        mut events: EventReader<crate::server::events::MessageEvent<StringMessage>>,
    ) {
        for event in events.read() {
            assert_eq!(event.message().0, "a".to_string());
            counter.0 += 1;
        }
    }

    #[test]
    fn client_send_message_as_host_server_client() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let mut stepper = HostServerStepper::default();

        stepper.server_app.init_resource::<Counter>();
        stepper.server_app.add_systems(Update, count_messages);

        // send a message from the local client to the server
        stepper
            .server_app
            .world_mut()
            .resource_mut::<ConnectionManager>()
            .send_message::<Channel1, StringMessage>(&StringMessage("a".to_string()))
            .unwrap();
        stepper.frame_step();
        stepper.frame_step();

        // verify that the server received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);
    }
}
