use crate::MessageManager;
use crate::registry::MessageRegistry;
use bevy_app::{App, Last, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::{Add, On};
use bevy_ecs::{
    schedule::{IntoScheduleConfigs, SystemSet},
    system::{ParamBuilder, Query, QueryParamBuilder, SystemParamBuilder},
};
use lightyear_connection::client::Disconnected;
use lightyear_transport::plugin::{TransportPlugin, TransportSystems};

#[deprecated(note = "Use MessageSystems instead")]
pub type MessageSet = MessageSystems;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum MessageSystems {
    // PRE UPDATE
    /// Receive Bytes from the Transport, deserialize them into Messages
    /// and buffer those in the [`MessageReceiver<M>`](crate::receive::MessageReceiver)
    Receive,

    // PostUpdate
    /// Receive messages from the [`MessageSender<M>`](crate::send::MessageSender), serialize them into Bytes
    /// and buffer those in the Transport
    Send,
}

// PLUGIN
// recv-messages: query all Transport + MessageManager
//  MessageManager is similar to transport, it holds references to MessageReceiver<M> and MessageSender<M> component ids
pub struct MessagePlugin;

impl MessagePlugin {
    // TODO: do something similar to Transport? (use observers instead of required_components)?
    /// On disconnect:
    /// - Reset the MessageManager to its original state
    fn handle_disconnection(
        trigger: On<Add, Disconnected>,
        mut manager_query: Query<&mut MessageManager>,
    ) {
        if let Ok(mut manager) = manager_query.get_mut(trigger.entity) {
            manager.entity_mapper.clear();
        }
    }
}

impl Plugin for MessagePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TransportPlugin>() {
            app.add_plugins(TransportPlugin);
        }

        app.add_observer(Self::handle_disconnection);

        #[cfg(feature = "client")]
        app.register_required_components::<lightyear_connection::client::Client, MessageManager>();

        #[cfg(feature = "server")]
        app.register_required_components::<lightyear_connection::prelude::server::ClientOf, MessageManager>();
    }

    // NOTE: this should only be called once all messages are registered, because we use the list of registered
    //  messages to provide the dynamic access
    fn finish(&self, app: &mut App) {
        let registry = app
            .world_mut()
            .remove_resource::<MessageRegistry>()
            .unwrap();

        let recv = (
            ParamBuilder,
            ParamBuilder,
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    registry.receive_metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.component_id);
                    });
                });
            }),
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(Self::recv)
            .with_name("MessagePlugin::recv");

        let clear = (
            ParamBuilder,
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    registry.receive_metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.component_id);
                    });
                });
            }),
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(Self::clear)
            .with_name("MessagePlugin::clear");

        let send = (
            ParamBuilder,
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    registry.send_metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.component_id);
                    });
                    registry
                        .send_trigger_metadata
                        .values()
                        .for_each(|metadata| {
                            b.mut_id(metadata.component_id);
                        });
                });
            }),
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(Self::send)
            .with_name("MessagePlugin::send");

        let send_local = (
            ParamBuilder,
            ParamBuilder,
            QueryParamBuilder::new(|builder| {
                builder.optional(|b| {
                    registry.send_metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.component_id);
                    });
                    registry.receive_metadata.values().for_each(|metadata| {
                        b.mut_id(metadata.component_id);
                    });
                    registry
                        .send_trigger_metadata
                        .values()
                        .for_each(|metadata| {
                            b.mut_id(metadata.component_id);
                        });
                });
            }),
            ParamBuilder,
            ParamBuilder,
        )
            .build_state(app.world_mut())
            .build_system(Self::send_local)
            .with_name("MessagePlugin::send_local");

        app.configure_sets(
            PreUpdate,
            MessageSystems::Receive.after(TransportSystems::Receive),
        );
        app.configure_sets(
            PostUpdate,
            MessageSystems::Send.before(TransportSystems::Send),
        );
        app.add_systems(PreUpdate, recv.in_set(MessageSystems::Receive));
        app.add_systems(PostUpdate, send.in_set(MessageSystems::Send));
        // we need to send local messages after clear, otherwise they will be cleared immediately after sending
        app.add_systems(Last, (clear, send_local).chain());

        app.world_mut().insert_resource(registry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receive::{MessageReceiver, ReceivedMessage};
    use crate::registry::AppMessageExt;
    use crate::send::MessageSender;
    use lightyear_connection::client::Connected;
    use lightyear_core::id::{PeerId, RemoteId};
    use lightyear_core::plugin::CorePlugins;
    use lightyear_core::prelude::{LocalTimeline, Tick};
    use lightyear_link::{Link, Linked};
    use lightyear_transport::channel::ChannelKind;
    use lightyear_transport::plugin::TestChannel;
    use lightyear_transport::plugin::TestTransportPlugin;
    use lightyear_transport::prelude::{ChannelRegistry, Transport};
    use serde::{Deserialize, Serialize};
    use test_log::test;

    /// Message
    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    struct M(usize);

    // TODO: should we do a test without the Link?

    /// Check that if we have a Transport, we can send and receive messages to specific channels
    #[test]
    fn test_send_receive() {
        let mut app = App::new();
        app.add_plugins(CorePlugins {
            tick_duration: core::time::Duration::from_millis(10),
        });
        app.add_plugins(TestTransportPlugin);

        // Register the message before adding the MessagePlugin
        app.register_message::<M>();
        app.add_plugins(MessagePlugin);
        app.finish();

        // Add the Transport component with a receiver/sender for channel C, and a receiver/sender for message M
        let registry = app.world().resource::<ChannelRegistry>();
        let mut transport = Transport::default();
        transport.add_sender_from_registry::<TestChannel>(registry);
        transport.add_receiver_from_registry::<TestChannel>(registry);
        // TODO: are these tests useful? they need so many components from other plugins..
        let mut entity_mut = app.world_mut().spawn((
            Link::default(),
            transport,
            MessageReceiver::<M>::default(),
            MessageSender::<M>::default(),
            LocalTimeline::default(),
            RemoteId(PeerId::Local(0)),
            Linked,
            Connected,
        ));

        let entity = entity_mut.id();

        // send message
        let message = M(2);
        entity_mut
            .get_mut::<MessageSender<M>>()
            .unwrap()
            .send::<TestChannel>(message.clone());
        app.update();

        // check that the send-payload was added to the Transport
        let mut entity_mut = app.world_mut().entity_mut(entity);
        let mut link = entity_mut.get_mut::<Link>().unwrap();
        assert_eq!(link.send.len(), 1);

        // transfer that payload to the recv side of the link
        let payload = link.send.pop().unwrap();
        link.recv.push_raw(payload);

        app.world_mut().run_schedule(PreUpdate);

        // check that the message has been received
        let received_message = app
            .world_mut()
            .entity_mut(entity)
            .get_mut::<MessageReceiver<M>>()
            .unwrap()
            .receive()
            .next()
            .expect("expected to receive message");
        assert_eq!(message, received_message);

        app.update();

        // check that the message has been dropped
        assert!(
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<MessageReceiver<M>>()
                .unwrap()
                .recv
                .is_empty()
        );
    }

    /// Check that messages are cleared even if not read
    #[test]
    fn test_clear() {
        let mut app = App::new();
        app.add_plugins(CorePlugins {
            tick_duration: core::time::Duration::from_millis(10),
        });
        app.register_message::<M>();
        app.add_plugins(MessagePlugin);
        app.finish();

        let entity_mut = app.world_mut().spawn((
            MessageReceiver::<M>::default(),
            RemoteId(PeerId::Local(0)),
            Connected,
        ));

        let entity = entity_mut.id();

        app.world_mut()
            .entity_mut(entity)
            .get_mut::<MessageReceiver<M>>()
            .unwrap()
            .recv
            .push(ReceivedMessage {
                data: M(2),
                remote_tick: Tick::default(),
                channel_kind: ChannelKind::of::<TestChannel>(),
                message_id: None,
            });
        app.update();

        // check that the message has been dropped
        assert!(
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<MessageReceiver<M>>()
                .unwrap()
                .recv
                .is_empty()
        );
    }
}
