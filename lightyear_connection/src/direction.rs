use bevy::ecs::entity::MapEntities;
use bevy::prelude::{App, Component, Event};
use lightyear_serde::ToBytes;

#[derive(Clone, Copy, PartialEq, Debug)]
/// [`NetworkDirection`] specifies in which direction the packets can be sent
pub enum NetworkDirection {
    ClientToServer,
    ServerToClient,
    Bidirectional,
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Client;
    use crate::client_of::ClientOf;
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
        })
            .add_direction(NetworkDirection::ClientToServer);
        app.add_channel::<ChannelServerToClient>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        })
            .add_direction(NetworkDirection::ServerToClient);
         app.add_channel::<ChannelBidirectional>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        })
             .add_direction(NetworkDirection::Bidirectional);

        let entity_mut = app.world_mut().spawn(Client);
        let transport = entity_mut.get::<Transport>().unwrap();

        transport.has_sender::<ChannelClientToServer>();
        transport.has_receiver::<ChannelServerToClient>();
        transport.has_sender::<ChannelBidirectional>();
        transport.has_receiver::<ChannelBidirectional>();

        let entity_mut = app.world_mut().spawn(ClientOf{
            server: Entity::PLACEHOLDER,
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

        MessageRegistration::<MessageClientToServer>::new(&mut app).add_direction(NetworkDirection::ClientToServer);
        MessageRegistration::<MessageServerToClient>::new(&mut app).add_direction(NetworkDirection::ServerToClient);
        MessageRegistration::<MessageBidirectional>::new(&mut app).add_direction(NetworkDirection::Bidirectional);

        let entity_mut = app.world_mut().spawn(Client);
        entity_mut.get::<MessageSender<MessageClientToServer>>().unwrap();
        entity_mut.get::<MessageReceiver<MessageServerToClient>>().unwrap();
        entity_mut.get::<MessageSender<MessageBidirectional>>().unwrap();
        entity_mut.get::<MessageReceiver<MessageBidirectional>>().unwrap();

        let entity_mut = app.world_mut().spawn(ClientOf{
            server: Entity::PLACEHOLDER,
        });
        entity_mut.get::<MessageReceiver<MessageClientToServer>>().unwrap();
        entity_mut.get::<MessageSender<MessageServerToClient>>().unwrap();
        entity_mut.get::<MessageSender<MessageBidirectional>>().unwrap();
        entity_mut.get::<MessageReceiver<MessageBidirectional>>().unwrap();
    }
}