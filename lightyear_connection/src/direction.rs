use crate::prelude::PeerId;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{App, Component};
use lightyear_messages::prelude::AppMessageExt;
use lightyear_messages::receive::MessageReceiver;
use lightyear_messages::registry::MessageRegistration;
use lightyear_messages::send::MessageSender;
use lightyear_messages::Message;
use lightyear_serde::ToBytes;
use lightyear_transport::channel::registry::ChannelRegistration;
use lightyear_transport::channel::Channel;

#[derive(Clone, Copy, PartialEq, Debug)]
/// [`NetworkDirection`] specifies in which direction the packets can be sent
pub enum NetworkDirection {
    ClientToServer,
    ServerToClient,
    Bidirectional,
}


pub trait AppMessageDirectionExt {
    /// Add a new [`NetworkDirection`] to the registry
    fn add_direction(&mut self, direction: NetworkDirection);

}

impl<M: Message> AppMessageDirectionExt for MessageRegistration<'_, M> {
    // TODO: as much as possible, don't include server code for dedicated clients and vice-versa
    //   see how we can achieve this. Maybe half of the funciton is in lightyear_client and the other half in lightyear_server ?
    fn add_direction(&mut self, direction: NetworkDirection) {
        #[cfg(feature = "client")]
        <Self as crate::client::AppMessageDirectionExt>::add_direction(self, direction);
        #[cfg(feature = "server")]
        <Self as crate::server::AppMessageDirectionExt>::add_direction(self, direction);
    }
}

pub trait AppChannelDirectionExt {
    fn add_direction(&mut self, direction: NetworkDirection);
}

impl<C: Channel> AppChannelDirectionExt for ChannelRegistration<'_, C> {
    /// Add a new [`NetworkDirection`] to the registry
    fn add_direction(&mut self, direction: NetworkDirection) {
        #[cfg(feature = "client")]
        <Self as crate::client::AppChannelDirectionExt>::add_direction(self, direction);
        #[cfg(feature = "server")]
        <Self as crate::server::AppChannelDirectionExt>::add_direction(self, direction);
    }
}


// pub trait AppComponentDirectionExt {
//     fn add_direction(&mut self, direction: NetworkDirection);
// }
//
// impl<C: Component> AppComponentDirectionExt for ComponentRegistration<'_, C> {
//     /// Add a new [`NetworkDirection`] to the registry
//     fn add_direction(&mut self, direction: NetworkDirection) {
//         #[cfg(feature = "client")]
//         <Self as crate::client::AppChannelDirectionExt>::add_direction(self, direction);
//         #[cfg(feature = "server")]
//         <Self as crate::server::AppChannelDirectionExt>::add_direction(self, direction);
//     }
// }





#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::PeerId;
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
            id: PeerId::Server,
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
            id: PeerId::Server,
        });
        entity_mut.get::<MessageReceiver<MessageClientToServer>>().unwrap();
        entity_mut.get::<MessageSender<MessageServerToClient>>().unwrap();
        entity_mut.get::<MessageSender<MessageBidirectional>>().unwrap();
        entity_mut.get::<MessageReceiver<MessageBidirectional>>().unwrap();
    }
}