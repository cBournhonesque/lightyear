use crate::Message;
use crate::prelude::{MessageReceiver, MessageSender};
use crate::registry::MessageRegistration;
use crate::send_trigger::TriggerSender;
use crate::trigger::TriggerRegistration;
use bevy::prelude::Event;
use lightyear_connection::client::Client;
use lightyear_connection::direction::NetworkDirection;

impl<M: Message> MessageRegistration<'_, M> {
    pub(crate) fn add_client_direction(&mut self, direction: NetworkDirection) {
        match direction {
            NetworkDirection::ClientToServer => {
                self.app
                    .register_required_components::<Client, MessageSender<M>>();
            }
            NetworkDirection::ServerToClient => {
                self.app
                    .register_required_components::<Client, MessageReceiver<M>>();
            }
            NetworkDirection::Bidirectional => {
                self.add_client_direction(NetworkDirection::ClientToServer);
                self.add_client_direction(NetworkDirection::ServerToClient);
            }
        }
    }
}

impl<M: Event> TriggerRegistration<'_, M> {
    pub(crate) fn add_client_direction(&mut self, direction: NetworkDirection) {
        match direction {
            NetworkDirection::ClientToServer => {
                self.app
                    .register_required_components::<Client, TriggerSender<M>>();
            }
            NetworkDirection::ServerToClient => {}
            NetworkDirection::Bidirectional => {
                self.add_client_direction(NetworkDirection::ClientToServer);
                self.add_client_direction(NetworkDirection::ServerToClient);
            }
        }
    }
}
