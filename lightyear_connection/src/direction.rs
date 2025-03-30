use crate::client::Client;
use crate::server::ClientOf;
use bevy::prelude::App;
use lightyear_messages::receive::MessageReceiver;
use lightyear_messages::send::MessageSender;
use lightyear_messages::Message;
use lightyear_transport::channel::Channel;

#[derive(Clone, Copy, PartialEq, Debug)]
/// [`NetworkDirection`] specifies in which direction the packets can be sent
pub enum NetworkDirection {
    ClientToServer,
    ServerToClient,
    Bidirectional,
}


pub trait AppDirectionExt {
    /// Add a new [`NetworkDirection`] to the registry
    fn add_message_direction<M: Message>(&mut self, direction: NetworkDirection);

     fn add_channel_direction<C: Channel>(&mut self, direction: NetworkDirection);
}

impl AppDirectionExt for App {
    // TODO: as much as possible, don't include server code for dedicated clients and vice-versa
    //   see how we can achieve this. Maybe half of the funciton is in lightyear_client and the other half in lightyear_server ?
    fn add_message_direction<M: Message>(&mut self, direction: NetworkDirection) {
        match direction {
            NetworkDirection::ClientToServer => {
                self.register_required_components::<Client, MessageSender<M>>();
                self.register_required_components::<ClientOf, MessageReceiver<M>>();
            }
            NetworkDirection::ServerToClient => {
                self.register_required_components::<Client, MessageReceiver<M>>();
                self.register_required_components::<ClientOf, MessageSender<M>>();
            }
            NetworkDirection::Bidirectional => {
                self.add_message_direction::<M>(NetworkDirection::ClientToServer);
                self.add_message_direction::<M>(NetworkDirection::ServerToClient);
            }
        }
    }

     /// Add a new [`NetworkDirection`] to the registry
    fn add_channel_direction<C: Channel>(&mut self, direction: NetworkDirection) {
         match direction {
            NetworkDirection::ClientToServer => {
                self.add_observer(Client::add_sender_channel::<C>);
                self.add_observer(ClientOf::add_receiver_channel::<C>);
            }
            NetworkDirection::ServerToClient => {
                self.add_observer(Client::add_receiver_channel::<C>);
                self.add_observer(ClientOf::add_sender_channel::<C>);
            }
            NetworkDirection::Bidirectional => {
                self.add_channel_direction::<C>(NetworkDirection::ClientToServer);
                self.add_channel_direction::<C>(NetworkDirection::ServerToClient);
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::ClientId;
    use bevy::prelude::{default, Entity};
    use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelRegistry, ChannelSettings, Transport};

    struct ChannelClientToServer;

    struct ChannelServerToClient;

    struct ChannelBidirectional;

    #[test]
    fn test_channel_direction() {
        let mut app = App::new();

        app.init_resource::<ChannelRegistry>();
        app.add_channel::<ChannelClientToServer>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        });
        app.add_channel_direction::<ChannelClientToServer>(NetworkDirection::ClientToServer);
        app.add_channel::<ChannelServerToClient>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        });
        app.add_channel_direction::<ChannelServerToClient>(NetworkDirection::ServerToClient);
         app.add_channel::<ChannelBidirectional>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        });
        app.add_channel_direction::<ChannelBidirectional>(NetworkDirection::Bidirectional);

        let entity_mut = app.world_mut().spawn(Client);
        let transport = entity_mut.get::<Transport>().unwrap();

        transport.has_sender::<ChannelClientToServer>();
        transport.has_receiver::<ChannelServerToClient>();
        transport.has_sender::<ChannelBidirectional>();
        transport.has_receiver::<ChannelBidirectional>();

        let entity_mut = app.world_mut().spawn(ClientOf{
            server: Entity::PLACEHOLDER,
            id: ClientId::Server,
        });
        let transport = entity_mut.get::<Transport>().unwrap();

        transport.has_receiver::<ChannelClientToServer>();
        transport.has_sender::<ChannelServerToClient>();
        transport.has_sender::<ChannelBidirectional>();
        transport.has_receiver::<ChannelBidirectional>();
    }
    
    struct MessageClientToServer;

    struct MessageServerToClient;

    struct MessageBidirectional;

    #[test]
    fn test_message_direction() {
        let mut app = App::new();


        app.add_message_direction::<MessageClientToServer>(NetworkDirection::ClientToServer);
        app.add_message_direction::<MessageServerToClient>(NetworkDirection::ServerToClient);
        app.add_message_direction::<MessageBidirectional>(NetworkDirection::Bidirectional);

        let entity_mut = app.world_mut().spawn(Client);
        entity_mut.get::<MessageSender<MessageClientToServer>>().unwrap();
        entity_mut.get::<MessageReceiver<MessageServerToClient>>().unwrap();
        entity_mut.get::<MessageSender<MessageBidirectional>>().unwrap();
        entity_mut.get::<MessageReceiver<MessageBidirectional>>().unwrap();

        let entity_mut = app.world_mut().spawn(ClientOf{
            server: Entity::PLACEHOLDER,
            id: ClientId::Server,
        });
        entity_mut.get::<MessageReceiver<MessageClientToServer>>().unwrap();
        entity_mut.get::<MessageSender<MessageServerToClient>>().unwrap();
        entity_mut.get::<MessageSender<MessageBidirectional>>().unwrap();
        entity_mut.get::<MessageReceiver<MessageBidirectional>>().unwrap();
    }
}