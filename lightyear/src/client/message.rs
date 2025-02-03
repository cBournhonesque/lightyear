//! Defines the [`ClientMessage`] enum used to send messages from the client to the server

use bevy::prelude::{App, Commands, IntoSystemConfigs, Mut, Plugin, PreUpdate, ResMut, World};
use byteorder::WriteBytesExt;
use bytes::Bytes;
use tracing::error;

use crate::client::connection::ConnectionManager;
use crate::prelude::{client::is_connected, ClientId};
use crate::protocol::message::MessageRegistry;
use crate::serialize::reader::Reader;
use crate::serialize::{SerializationError, ToBytes};
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{ClientMarker, InternalMainSet};

pub struct ClientMessagePlugin;

impl Plugin for ClientMessagePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            read_messages
                .in_set(InternalMainSet::<ClientMarker>::EmitEvents)
                .run_if(is_connected),
        );
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

/// Read the messages received from the server and handle them:
/// - Messages: send a MessageEvent
/// - Events: send them to EventWriter or trigger them
fn read_messages(mut commands: Commands, mut connection: ResMut<ConnectionManager>) {
    connection
        .received_messages
        .iter_mut()
        .for_each(|(net_id, message_list)| {
            message_list.drain(..).for_each(|message| {
                let mut reader = Reader::from(message);
                // make copies to avoid `connection` to be moved inside the closure
                let net_id = *net_id;
                commands.queue(move |world: &mut World| {
                    // NOTE: removing the resources is a bit risky... however we use the world
                    // only to get the Events<MessageEvent<M>> so it should be ok
                    world.resource_scope(|world, registry: Mut<MessageRegistry>| {
                        world.resource_scope(|world, mut manager: Mut<ConnectionManager>| {
                            let _ = registry
                                // we have to re-decode the net id
                                .receive_message(
                                    net_id,
                                    world,
                                    // TODO: include the client that rebroadcasted the message?
                                    ClientId::Local(0),
                                    &mut reader,
                                    &mut manager
                                        .replication_receiver
                                        .remote_entity_map
                                        .remote_to_local,
                                )
                                .inspect_err(|e| error!("Could not deserialize message: {:?}", e));
                        })
                    });
                });
            })
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
