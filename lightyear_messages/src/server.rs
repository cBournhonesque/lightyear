use crate::prelude::{MessageReceiver, MessageSender};
use crate::registry::MessageRegistration;
use crate::send_trigger::TriggerSender;
use crate::trigger::TriggerRegistration;
use crate::Message;
use bevy::prelude::Event;
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::direction::NetworkDirection;

impl<M: Message> MessageRegistration<'_, M> {
    pub(crate) fn add_server_direction(&mut self, direction: NetworkDirection) {
        match direction {
            NetworkDirection::ClientToServer => {
                self.app.register_required_components::<ClientOf, MessageSender<M>>();
            }
            NetworkDirection::ServerToClient => {
                self.app.register_required_components::<ClientOf, MessageReceiver<M>>();
            }
            NetworkDirection::Bidirectional => {
                self.add_server_direction(NetworkDirection::ClientToServer);
                self.add_server_direction(NetworkDirection::ServerToClient);
            }
        }
    }
}

impl<M: Event> TriggerRegistration<'_, M> {
    pub(crate) fn add_server_direction(&mut self, direction: NetworkDirection) {
        match direction {
            NetworkDirection::ClientToServer => {}
            NetworkDirection::ServerToClient => {
                self.app.register_required_components::<ClientOf, TriggerSender<M>>();
            }
            NetworkDirection::Bidirectional => {
                self.add_server_direction(NetworkDirection::ClientToServer);
                self.add_server_direction(NetworkDirection::ServerToClient);
            }
        }
    }
}