// Current design:
// - you call `Events<ToServer<SendMessage<M>>>` to send a message
// - we have a type-erased system that reads from all these events, serializes and stores them
//   in the MessageManager

// Each Link/Transport entity.
// - You can call `ChannelSender::<C>::send_message::<M>()`
// - We

// Should the EntityMap be stored on the Link? or on the Transport?
// Or a different component?

// Borrowing API:
// - You call ChannelSender::<C>.send_message::<M>(
//     &MessageRegistry,
//     &mut EntityMapper,  -> taken from the entity?
//     &mut Transport,  -> taken from the entity?
//   )
//
// And it will:
// - call the message registry to serialize your bytes
// - call the entity mapper to map your entities
// - buffer the bytes on ChannelSenderEnum contained in the Sender

// Non-Borrowing API (Events):
// - You create a message `SendMessage<M>::new::<C>(message, entity)`
// - You call `Events<SendMessage<M>>` to send a message
// - One system reads from all these events and using the entity + channel_kind
//   calls the correct type-erased `ChannelSender::<C>::send_message::<M>()`
//   with the correct EntityMapper and Serializer
//
// Instead of `entity` as second-argument, you could provide `NetworkTarget`,
// and we will find the correct entities that correspond to this target.
// We could have a trait `ToTransportEntity` implemented for Entity, Vec<Entity>, NetworkTarget, etc.

// TODO: how to do re-broadcasts?
// - on server->client we don't want rebroadcasts
// - on client->server, we want rebroadcasts.

// - Or maybe we have normal Messages that are broadcasted.
//   And then users could wrap messages with RebroadcastMessage<M>

// - messages are registered with a ChannelDirection,
//   but on the Transport we can add a Direction component (
//   and we only go through Events if the direction matches

// - Transport component has a Direction (RecvOnly/SendOnly/Both)
// - public-facing MessageRegistry has a ClientToServer/ServerToClient/Bidirectional (in lightyear)
// - lightyear_messages should be independent from client or server. It's just a way to send typed
//   data over a Transport
//    - based on MessageRegistry ClientToServer + Transport's Identity (Client/Server/HostServer) and Direction, we can figure out SendMessageMetadata/ReceiveMessageMetadata to add receive on the Transport
// TODO:
//  - should we have a separate MessageTransport component? or should the messages be part of the Transport itself? Maybe it's cleaner to have a separate Component?
//  - in this design Messages require Channels, so maybe lightyear_message should be a 'message' folder that is part of lightyear_transport?
// i.e order is:
// - Register Channels in lighyear with ChannelDirection (ClientToServer/ServerToClient/Bidirectional)
//   - this registers Channels in lightyear_transport without the direction? and a separate registry
//     keeps track of the direction?
// - Register Messages with ChannelDirection (ClientToServer/ServerToClient/Bidirectional)
// - You create an entity with an Client or Server component
//   - Identity becomes HostServer if Client and Server are present on the same component
//   - ClientOf automatically adds the Client marker component
// - When you create a Transport, you look at the Client and Server component to determine the Identity
//   and to determine which ChannelSenders/Receivers to add to the entity.
//   - the lightyear plugin will add all the ChannelSenders<C> depending on the direction + also update
//     the Receiver metadata; but that's lightyear. Lightyear_transport is client/server agnostic
//   - That's for the 'official' entity. But users could create their own entity where they only add a subset of
//     the channels?
//   - same for messages.
// - lightyear_transport is agnostic to direction:
//   - it only registers the channel setting independently from direction.
//   - users add receivers to the Transport manually (need to provide the ComponentKind and the ReceiverEnum(with settings)). That's annoying because ideally we could just provide the ComponentKind and some observer fetches the settings from the registry? Maybe a ChannelReceiver<C> marker component that triggers an observer?
//   - users can add senders to the Transport by adding a ChannelSender<C>, whose on_add observer will
//     get the ComponentId, fetch the settings from the registry, etc.
// - Same thing lightyear_transport is agnostic to direction for messages:
//   - users can add ReceiveMessage<M> (by simply providing the ComponentKind) on the Transport
//   - users can add SendMessage<M> (by simply providing the ComponentKind)
// - ReceiveMessage:
//   - get the Bytes from the Transport Receiver (so we know the ChannelId)
//   - we deserialize them, etc.
//   - we push the message via type-erasure to the ReceivedMessage<M> component
//     - if we're the client, we know it's from the server? (and RebroadcastMessage<M> would include the identity of the original sender)
//     - if we're the server, we are on the ClientOf entity, so we know which client sent us the data?
//
//   So we always want to receive on components, not events.

// For performance I was splitting up Sender<M> and Sender<C> but maybe that's not needed?
// - instead: MessageManager has a type-erased crossbeam Sender<(M, ChannelKind, Priority)>
//      that users can write to in parallel
//   - we have a type-erased fn that reads from the Receiver<M>, serializes it, and writes to the Transports Sender<(C, Bytes)>

// If you specify that rebroadcast is allowed, we will also register RebroadcastMessage<M> in the registry!
//   - For rebroadcasting we will let the server deserialize the message to inspect the contents and do validation?

// TODO: provide an api where we send to the link directly?

// // Extension trait so that we can implement it for ChannelSender<C>
// trait SendMessage<M: Message> {
//     fn send_message<M>(
//         &mut self,
//         message: M,
//         priority: f32,
//         registry: &MessageRegistry,
//         transport: &mut Transport,
//         // TODO: separate error type for SendMessage and ReceiveMessage
//     ) -> Result<(), MessageError>;
// }
//
// impl<C: Channel, M: Message> SendMessage<M> for ChannelSender<C> {
//
//     fn send_message<M>(&mut self, message: &M, priority: f32, registry: &MessageRegistry, transport: &mut Transport) -> Result<(), MessageError> {
//         registry.serialize(message, &mut self.writer, &mut transport.send_mapper)?;
//         let message_id = self.sender.buffer_send(self.writer.split(), priority)?;
//         Ok(())
//     }
// }

use crate::MessageManager;
use crate::registry::MessageRegistry;
use bevy::app::{App, Last, PostUpdate, PreUpdate};
use bevy::ecs::system::{ParamBuilder, QueryParamBuilder};
use bevy::prelude::*;
use lightyear_connection::client::Disconnected;
use lightyear_transport::plugin::{TransportPlugin, TransportSet};

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum MessageSet {
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
        trigger: Trigger<OnAdd, Disconnected>,
        mut manager_query: Query<&mut MessageManager>,
    ) {
        if let Ok(mut manager) = manager_query.get_mut(trigger.target()) {
            manager.entity_mapper.clear();
        }
    }
}

impl Plugin for MessagePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<MessageManager>();

        if !app.is_plugin_added::<TransportPlugin>() {
            app.add_plugins(TransportPlugin);
        }

        app.add_observer(Self::handle_disconnection);

        #[cfg(feature = "client")]
        app.register_required_components::<lightyear_connection::prelude::Client, MessageManager>();

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
            .build_system(Self::recv);

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
            .build_system(Self::clear);

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
            .build_system(Self::send);

        let send_local = (
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
            .build_system(Self::send_local);

        app.configure_sets(PreUpdate, MessageSet::Receive.after(TransportSet::Receive));
        app.configure_sets(PostUpdate, MessageSet::Send.before(TransportSet::Send));
        app.add_systems(PreUpdate, recv.in_set(MessageSet::Receive));
        app.add_systems(PostUpdate, send.in_set(MessageSet::Send));
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
    use lightyear_core::id::{PeerId, RemoteId};
    use lightyear_core::plugin::CorePlugins;
    use lightyear_core::prelude::{LocalTimeline, Tick};
    use lightyear_link::{Link, Linked};
    use lightyear_transport::channel::ChannelKind;
    use lightyear_transport::plugin::tests::C;
    use lightyear_transport::plugin::tests::TestTransportPlugin;
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
        app.add_message::<M>();
        app.add_plugins(MessagePlugin);
        app.finish();

        // Add the Transport component with a receiver/sender for channel C, and a receiver/sender for message M
        let registry = app.world().resource::<ChannelRegistry>();
        let mut transport = Transport::default();
        transport.add_sender_from_registry::<C>(registry);
        transport.add_receiver_from_registry::<C>(registry);
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
            .send::<C>(message.clone());
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
        app.add_message::<M>();
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
                channel_kind: ChannelKind::of::<C>(),
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
