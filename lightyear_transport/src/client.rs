use crate::channel::Channel;
use crate::channel::registry::ChannelRegistration;
use crate::prelude::{ChannelRegistry, Transport};
use bevy_ecs::{
    observer::Trigger,
    query::With,
    system::{Query, Res},
    world::Insert,
};
use lightyear_connection::client::Client;
use lightyear_connection::direction::NetworkDirection;

pub(crate) fn add_sender_channel<C: Channel>(
    trigger: On<Insert, (Transport, Client)>,
    mut query: Query<&mut Transport, With<Client>>,
    registry: Res<ChannelRegistry>,
) {
    if let Ok(mut transport) = query.get_mut(trigger.entity) {
        transport.add_sender_from_registry::<C>(&registry)
    }
}

pub(crate) fn add_receiver_channel<C: Channel>(
    trigger: On<Insert, (Transport, Client)>,
    mut query: Query<&mut Transport, With<Client>>,
    registry: Res<ChannelRegistry>,
) {
    if let Ok(mut transport) = query.get_mut(trigger.entity) {
        transport.add_receiver_from_registry::<C>(&registry)
    }
}

impl<C: Channel> ChannelRegistration<'_, C> {
    pub(crate) fn add_client_direction(&mut self, direction: NetworkDirection) {
        match direction {
            NetworkDirection::ClientToServer => {
                self.app.add_observer(add_sender_channel::<C>);
            }
            NetworkDirection::ServerToClient => {
                self.app.add_observer(add_receiver_channel::<C>);
            }
            NetworkDirection::Bidirectional => {
                self.add_client_direction(NetworkDirection::ClientToServer);
                self.add_client_direction(NetworkDirection::ServerToClient);
            }
        }
    }
}
