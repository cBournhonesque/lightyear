use crate::channel::registry::ChannelRegistration;
use crate::channel::Channel;
use crate::prelude::{ChannelRegistry, Transport};
use bevy::prelude::{OnAdd, Query, Res, Trigger};
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::direction::NetworkDirection;

pub(crate) fn add_sender_channel<C: Channel>(trigger: Trigger<OnAdd, ClientOf>, mut query: Query<&mut Transport>, registry: Res<ChannelRegistry>) {
    if let Ok(mut transport) = query.get_mut(trigger.target()) {
        transport.add_sender_from_registry::<C>(&registry)
    }
}

pub(crate) fn add_receiver_channel<C: Channel>(trigger: Trigger<OnAdd, ClientOf>, mut query: Query<&mut Transport>, registry: Res<ChannelRegistry>) {
    if let Ok(mut transport) = query.get_mut(trigger.target()) {
        transport.add_receiver_from_registry::<C>(&registry)
    }
}

impl<C: Channel> ChannelRegistration<'_, C> {
    /// Add a new [`NetworkDirection`] to the registry
    pub(crate) fn add_server_direction(&mut self, direction: NetworkDirection) {
         match direction {
            NetworkDirection::ClientToServer => {
                self.app.add_observer(add_sender_channel::<C>);
            }
            NetworkDirection::ServerToClient => {
                self.app.add_observer(add_receiver_channel::<C>);
            }
            NetworkDirection::Bidirectional => {
                self.add_server_direction(NetworkDirection::ClientToServer);
                self.add_server_direction(NetworkDirection::ServerToClient);
            }
        }
    }
}