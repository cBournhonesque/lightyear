use std::ops::DerefMut;

use crate::prelude::{server::is_started, Message};
use crate::protocol::message::{MessageKind, MessageRegistry};
use crate::serialize::reader::Reader;
use crate::server::connection::ConnectionManager;
use crate::server::events::MessageEvent;
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{InternalMainSet, ServerMarker};
use bevy::app::{App, PreUpdate};
use bevy::prelude::{EventWriter, IntoSystemConfigs, Res, ResMut};
use tracing::{error, trace};

/// Read the messages received from the clients and emit the MessageEvent event
fn read_message<M: Message>(
    message_registry: Res<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
    mut event: EventWriter<MessageEvent<M>>,
) {
    let kind = MessageKind::of::<M>();
    let Some(net) = message_registry.kind_map.net_id(&kind).copied() else {
        error!(
            "Could not find the network id for the message kind: {:?}",
            kind
        );
        return;
    };
    // re-borrow to allow split borrows
    let connection_manager = connection_manager.deref_mut();
    for (client_id, connection) in connection_manager.connections.iter_mut() {
        if let Some(message_list) = connection.received_messages.remove(&net) {
            for (message_bytes, target, channel_kind) in message_list {
                let mut reader = Reader::from(message_bytes);
                match message_registry.deserialize::<M>(
                    &mut reader,
                    &mut connection
                        .replication_receiver
                        .remote_entity_map
                        .remote_to_local,
                ) {
                    Ok(message) => {
                        // rebroadcast
                        if target != NetworkTarget::None {
                            connection.messages_to_rebroadcast.push((
                                reader.consume(),
                                target,
                                channel_kind,
                            ));
                        }
                        event.send(MessageEvent::new(message, *client_id));
                        trace!("Received message: {:?}", std::any::type_name::<M>());
                    }
                    Err(e) => {
                        error!(
                            "Could not deserialize message {}: {:?}",
                            std::any::type_name::<M>(),
                            e
                        );
                    }
                }
            }
        }
    }
}

/// Register a message that can be sent from client to server
pub(crate) fn add_server_receive_message_from_client<M: Message>(app: &mut App) {
    app.add_event::<MessageEvent<M>>();
    app.add_systems(
        PreUpdate,
        read_message::<M>
            .in_set(InternalMainSet::<ServerMarker>::EmitEvents)
            .run_if(is_started),
    );
}

// impl ServerMessage {
//     pub(crate) fn emit_send_logs(&self, channel_name: &str) {
//         match self {
//             ServerMessage::Message(message) => {
//                 let message_name = message.name();
//                 trace!(channel = ?channel_name, message = ?message_name, kind = ?message.kind(), "Sending message");
//                 #[cfg(metrics)]
//                 metrics::counter!("send_message", "channel" => channel_name, "message" => message_name).increment(1);
//             }
//             ServerMessage::Replication(message) => {
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
//             ServerMessage::Sync(message) => match message {
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

// TODO: another option is to add ClientMessage and ServerMessage to ProtocolMessage
// then we can keep the shared logic in connection.mod. We just lose 1 bit everytime...

#[cfg(test)]
mod tests {
    use crate::prelude::ClientId;
    use crate::tests::host_server_stepper::{HostServerStepper, Step, LOCAL_CLIENT_ID};
    use crate::tests::protocol::{Channel1, Message1};
    use bevy::app::Update;
    use bevy::prelude::{EventReader, ResMut, Resource};

    #[derive(Resource, Default)]
    struct Counter(usize);

    /// System to check that we received the message on the server
    fn count_messages(
        mut counter: ResMut<Counter>,
        mut events: EventReader<crate::client::events::MessageEvent<Message1>>,
    ) {
        for event in events.read() {
            assert_eq!(event.message().0, "a".to_string());
            counter.0 += 1;
        }
    }

    /// In host-server mode, the server is sending a message to the local client
    #[test]
    fn server_send_message_to_local_client() {
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::ERROR)
            .init();
        let mut stepper = HostServerStepper::default();

        stepper.server_app.init_resource::<Counter>();
        stepper.server_app.add_systems(Update, count_messages);

        // send a message from the local client to the server
        stepper
            .server_app
            .world_mut()
            .resource_mut::<crate::prelude::server::ConnectionManager>()
            .send_message::<Channel1, Message1>(
                ClientId::Local(LOCAL_CLIENT_ID),
                &Message1("a".to_string()),
            )
            .unwrap();
        stepper.frame_step();
        stepper.frame_step();
        stepper.frame_step();
        stepper.frame_step();

        // verify that the server received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);
    }
}
