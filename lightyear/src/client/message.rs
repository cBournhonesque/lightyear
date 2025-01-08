//! Defines the [`ClientMessage`] enum used to send messages from the client to the server

use crate::client::commands::ClientCommands;
use crate::client::connection::ConnectionManager;
use crate::prelude::{client::is_connected, Channel, ChannelKind, ClientId, Message};
use crate::protocol::message::MessageRegistry;
use crate::serialize::reader::Reader;
use crate::serialize::{SerializationError, ToBytes};
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{ClientMarker, InternalMainSet};
use bevy::prelude::{App, Commands, IntoSystemConfigs, Mut, Plugin, PreUpdate, ResMut, World};
use byteorder::WriteBytesExt;
use bytes::Bytes;
use tracing::error;

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

pub trait ClientMessageExt: crate::shared::message::private::InternalMessageSend {
    fn send_message<C: Channel, M: Message>(&mut self, message: &M) {
        self.send_message_to_target::<C, M>(message, NetworkTarget::None)
    }

    fn send_message_to_target<C: Channel, M: Message>(
        &mut self,
        message: &M,
        target: NetworkTarget,
    ) {
        self.erased_send_message_to_target(message, ChannelKind::of::<C>(), target)
    }
}

impl ClientMessageExt for ClientCommands<'_, '_> {}

impl crate::shared::message::private::InternalMessageSend for ClientCommands<'_, '_> {
    fn erased_send_message_to_target<M: Message>(
        &mut self,
        message: &M,
        channel_kind: ChannelKind,
        target: NetworkTarget,
    ) {
        // TODO: HANDLE ERRORS
        self.queue(move |world: &mut World| {
            // TODO: fetch the entity that contains the Transport/Writer/MessageManager
            let Some(mut manager) = world.get_resource_mut::<ConnectionManager>() else {
                return;
            };
            world.resource_scope(|world, registry: Mut<MessageRegistry>| {
                let Some(registry) = world.get_resource::<MessageRegistry>() else {
                    return;
                };
                // write the target first
                // NOTE: this is ok to do because most of the time (without rebroadcast, this just adds 1 byte)

                let _ = target.to_bytes(&mut manager.writer);
                // then write the message
                let _ = registry.serialize(
                    message,
                    &mut manager.writer,
                    Some(
                        &mut manager
                            .replication_receiver
                            .remote_entity_map
                            .local_to_remote,
                    ),
                );
                let message_bytes = manager.writer.split();
                // TODO: emit logs/metrics about the message being buffered?
                manager.messages_to_send.push((message_bytes, channel_kind));
            });
        });
        // self.queue(|world: &mut World| {
        //     // TODO: fetch the entity that contains the Transport/Writer/MessageManager
        //     let Some(mut manager) = world.get_resource_mut::<ConnectionManager>() else {
        //         return Err(ConnectionError::NotFound.into());
        //     };
        //     let Some(registry) = world.get_resource::<MessageRegistry>() else {
        //         return Err(ConnectionError::NotFound.into());
        //     };
        //     // write the target first
        //     // NOTE: this is ok to do because most of the time (without rebroadcast, this just adds 1 byte)
        //     target.to_bytes(&mut manager.writer)?;
        //     // then write the message
        //     registry.serialize(
        //         message,
        //         &mut manager.writer,
        //         Some(
        //             &mut manager
        //                 .replication_receiver
        //                 .remote_entity_map
        //                 .local_to_remote,
        //         ),
        //     )?;
        //     let message_bytes = manager.writer.split();
        //     // TODO: emit logs/metrics about the message being buffered?
        //     manager.messages_to_send.push((message_bytes, channel_kind));
        //     Ok(())
        // });
    }
}

// impl ClientMessage {
//     pub(crate) fn emit_send_logs(&self, channel_name: &str) {
//         match self {
//             ClientMessage::Message(message, _) => {
//                 let message_name = message.name();
//                 trace!(channel = ?channel_name, message = ?message_name, kind = ?message.kind(), "Sending message");
//                 #[cfg(metrics)]
//                 metrics::counter!("send_message", "channel" => channel_name, "message" => message_name).increment(1);
//             }
//             ClientMessage::Replication(message) => {
//                 let _span = info_span!("send replication message", channel = ?channel_name, group_id = ?message.group_id);
//                 #[cfg(metrics)]
//                 metrics::counter!("send_replication_actions").increment(1);
//                 match &message.data {
//                     ReplicationMessageData::Actions(m) => {
//                         for (entity, actions) in &m.actions {
//                             let _span = info_span!("send replication actions", ?entity);
//                             if actions.spawn {
//                                 trace!("Send entity spawn");
//                                 #[cfg(metrics)]
//                                 metrics::counter!("send_entity_spawn").increment(1);
//                             }
//                             if actions.despawn {
//                                 trace!("Send entity despawn");
//                                 #[cfg(metrics)]
//                                 metrics::counter!("send_entity_despawn").increment(1);
//                             }
//                             if !actions.insert.is_empty() {
//                                 let components = actions
//                                     .insert
//                                     .iter()
//                                     .map(|c| c.into())
//                                     .collect::<Vec<P::ComponentKinds>>();
//                                 trace!(?components, "Sending component insert");
//                                 #[cfg(metrics)]
//                                 {
//                                     for component in components {
//                                         metrics::counter!("send_component_insert", "component" => kind).increment(1);
//                                     }
//                                 }
//                             }
//                             if !actions.remove.is_empty() {
//                                 trace!(?actions.remove, "Sending component remove");
//                                 #[cfg(metrics)]
//                                 {
//                                     for kind in actions.remove {
//                                         metrics::counter!("send_component_remove", "component" => kind).increment(1);
//                                     }
//                                 }
//                             }
//                             if !actions.updates.is_empty() {
//                                 let components = actions
//                                     .updates
//                                     .iter()
//                                     .map(|c| c.into())
//                                     .collect::<Vec<P::ComponentKinds>>();
//                                 trace!(?components, "Sending component update");
//                                 #[cfg(metrics)]
//                                 {
//                                     for component in components {
//                                         metrics::counter!("send_component_update", "component" => kind).increment(1);
//                                     }
//                                 }
//                             }
//                         }
//                     }
//                     ReplicationMessageData::Updates(m) => {
//                         for (entity, updates) in &m.updates {
//                             let _span = info_span!("send replication updates", ?entity);
//                             let components = updates
//                                 .iter()
//                                 .map(|c| c.into())
//                                 .collect::<Vec<P::ComponentKinds>>();
//                             trace!(?components, "Sending component update");
//                             #[cfg(metrics)]
//                             {
//                                 for component in components {
//                                     metrics::counter!("send_component_update", "component" => kind)
//                                         .increment(1);
//                                 }
//                             }
//                         }
//                     }
//                 }
//             }
//             ClientMessage::Sync(message) => match message {
//                 SyncMessage::Ping(_) => {
//                     trace!(channel = ?channel_name, "Sending ping");
//                     #[cfg(metrics)]
//                     metrics::counter!("send_ping", "channel" => channel_name).increment(1);
//                 }
//                 SyncMessage::Pong(_) => {
//                     trace!(channel = ?channel_name, "Sending pong");
//                     #[cfg(metrics)]
//                     metrics::counter!("send_pong", "channel" => channel_name).increment(1);
//                 }
//             },
//         }
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::client::*;
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
            .commands()
            .client()
            .send_message::<Channel1, StringMessage>(&StringMessage("a".to_string()));
        stepper.frame_step();
        stepper.frame_step();

        // verify that the server received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);
    }
}
