use crate::prelude::server::is_stopped;
use crate::protocol::message::{MessageRegistry, MessageType};
use crate::serialize::reader::Reader;
use crate::server::connection::ConnectionManager;
use crate::shared::replication::network_target::NetworkTarget;
use crate::shared::sets::{InternalMainSet, ServerMarker};
use bevy::app::{App, Plugin, PreUpdate};
use bevy::ecs::system::{FilteredResourcesMutParamBuilder, ParamBuilder};
use bevy::prelude::{
    not, Commands, FilteredResourcesMut, IntoSystemConfigs, ResMut, SystemParamBuilder,
};
use tracing::{error, trace};

/// Plugin that adds functionality related to receiving messages from clients
#[derive(Default)]
pub struct ServerMessagePlugin;

impl Plugin for ServerMessagePlugin {
    fn build(&self, app: &mut App) {}

    /// Add the system after all messages have been added to the MessageRegistry
    fn cleanup(&self, app: &mut App) {
        let message_registry = app
            .world_mut()
            .remove_resource::<MessageRegistry>()
            .unwrap();
        // Use FilteredResourceMut SystemParam to register the access dynamically to the
        // Messages in the MessageRegistry
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
                .in_set(InternalMainSet::<ServerMarker>::EmitEvents)
                .run_if(not(is_stopped)),
        );
        app.world_mut().insert_resource(message_registry);
    }
}

/// Read the messages received from the clients and emit the MessageEvent events
/// Also rebroadcast the messages if needed
fn read_messages(
    mut events: FilteredResourcesMut,
    mut commands: Commands,
    message_registry: ResMut<MessageRegistry>,
    mut connection_manager: ResMut<ConnectionManager>,
) {
    // re-borrow to allow split borrows
    for (client_id, connection) in connection_manager.connections.iter_mut() {
        connection.received_messages.drain(..).for_each(
            |(net_id, message_bytes, target, channel_kind)| {
                let mut reader = Reader::from(message_bytes);
                match message_registry.receive_message(
                    net_id,
                    &mut commands,
                    &mut events,
                    *client_id,
                    &mut reader,
                    &mut connection
                        .replication_receiver
                        .remote_entity_map
                        .remote_to_local,
                ) {
                    Ok(_) => {
                        // rebroadcast
                        if target != NetworkTarget::None {
                            connection.messages_to_rebroadcast.push((
                                reader.consume(),
                                target,
                                channel_kind,
                            ));
                        }
                        trace!("Received message! NetId: {net_id:?}");
                    }
                    Err(e) => {
                        error!("Could not deserialize message (NetId: {net_id:?}): {e:?}");
                    }
                }
            },
        );
    }
}

// TODO: another option is to add ClientMessage and ServerMessage to ProtocolMessage
// then we can keep the shared logic in connection.mod. We just lose 1 bit everytime...

#[cfg(test)]
mod tests {
    use crate::prelude::NetworkTarget;
    use crate::shared::message::MessageSend;
    use crate::tests::host_server_stepper::HostServerStepper;
    use crate::tests::protocol::{Channel1, StringMessage};
    use bevy::app::Update;
    use bevy::prelude::{EventReader, ResMut, Resource};

    #[derive(Resource, Default)]
    struct Counter(usize);

    /// System to check that we received the message on the server
    fn count_messages(
        mut counter: ResMut<Counter>,
        mut events: EventReader<crate::client::events::MessageEvent<StringMessage>>,
    ) {
        for event in events.read() {
            assert_eq!(event.message().0, "a".to_string());
            counter.0 += 1;
        }
    }

    /// In host-server mode, the server is sending a message to the local client
    #[test]
    fn server_send_message_to_local_client() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let mut stepper = HostServerStepper::default();

        stepper.server_app.init_resource::<Counter>();
        stepper.client_app.init_resource::<Counter>();
        stepper.server_app.add_systems(Update, count_messages);
        stepper.client_app.add_systems(Update, count_messages);

        // send a message from the host-server server to all clients
        stepper
            .server_app
            .world_mut()
            .resource_mut::<crate::prelude::server::ConnectionManager>()
            .send_message_to_target::<Channel1, StringMessage>(
                &StringMessage("a".to_string()),
                NetworkTarget::All,
            )
            .unwrap();
        stepper.frame_step();
        stepper.frame_step();
        stepper.frame_step();
        stepper.frame_step();

        // verify that the local-client received the message
        assert_eq!(stepper.server_app.world().resource::<Counter>().0, 1);

        // verify that the other client received the message
        assert_eq!(stepper.client_app.world().resource::<Counter>().0, 1);
    }
}
