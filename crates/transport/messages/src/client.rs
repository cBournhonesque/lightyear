use crate::Message;
use crate::prelude::MessageSender;
use crate::registry::MessageRegistration;
use crate::send_trigger::EventSender;
use crate::trigger::TriggerRegistration;
use bevy_ecs::event::Event;
use lightyear_connection::client::Client;
use lightyear_connection::direction::NetworkDirection;

impl<M: Message> MessageRegistration<'_, M> {
    pub(crate) fn add_client_direction(&mut self, direction: NetworkDirection) {
        match direction {
            NetworkDirection::ClientToServer => {
                self.app
                    .try_register_required_components::<Client, MessageSender<M>>()
                    .ok();
            }
            NetworkDirection::ServerToClient => {}
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
                    .try_register_required_components::<Client, EventSender<M>>()
                    .ok();
            }
            NetworkDirection::ServerToClient => {
                // Immediate events are triggered directly and need no receiver component.
            }
            NetworkDirection::Bidirectional => {
                self.add_client_direction(NetworkDirection::ClientToServer);
                self.add_client_direction(NetworkDirection::ServerToClient);
            }
        }
    }
}
