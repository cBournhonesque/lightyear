use crate::multi::MultiMessageSender;
use crate::prelude::{MessageReceiver, MessageSender};
use crate::registry::MessageRegistration;
use crate::send::Priority;
use crate::send_trigger::TriggerSender;
use crate::trigger::TriggerRegistration;
use crate::Message;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use lightyear_connection::client::PeerMetadata;
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_link::prelude::Server;
use lightyear_serde::entity_map::SendEntityMap;
use lightyear_transport::channel::Channel;
use tracing::error;

#[derive(SystemParam)]
pub struct ServerMultiMessageSender<'w, 's> {
    sender: MultiMessageSender<'w, 's>,
    metadata: Res<'w, PeerMetadata>,
}

impl ServerMultiMessageSender<'_, '_> {
    pub fn send<M: Message, C: Channel>(
        &mut self,
        message: &M,
        server: &Server,
        target: &NetworkTarget,
    ) -> Result {
        self.send_with_priority::<M, C>(message, server, target, 1.0)
    }

    pub fn send_with_priority<M: Message, C: Channel>(
        &mut self,
        message: &M,
        server: &Server,
        target: &NetworkTarget,
        priority: Priority,
    ) -> Result {
        // if the message is not map-entities, we can serialize it once and clone the bytes
        if !self.sender.registry.is_map_entities::<M>()? {
            // TODO: serialize once for all senders. Figure out how to get a shared writer. Maybe on Server? Or as a global resource?
            //   or as Local?
            self.sender.registry.serialize::<M>(
                message,
                &mut self.sender.writer,
                &mut SendEntityMap::default(),
            )?;
            let bytes = self.sender.writer.split();
            target.apply_targets(
                server.collection().iter().copied(),
                &self.metadata.mapping,
                &mut |sender| {
                    if let Ok((_, transport)) = self.sender.query.get(sender) {
                        transport
                            .send_with_priority::<C>(bytes.clone(), priority)
                            .inspect_err(|e| error!("Failed to send message: {e}"))
                            .ok();
                    }
                },
            );
        } else {
            target.apply_targets(
                server.collection().iter().copied(),
                &self.metadata.mapping,
                &mut |sender| {
                    if let Ok((_, transport)) = self.sender.query.get(sender) {
                        self.sender
                            .registry
                            .serialize::<M>(
                                message,
                                &mut self.sender.writer,
                                &mut SendEntityMap::default(),
                            )
                            .unwrap();
                        let bytes = self.sender.writer.split();
                        transport
                            .send_with_priority::<C>(bytes.clone(), priority)
                            .inspect_err(|e| error!("Failed to send message: {e}"))
                            .ok();
                    }
                },
            );
        }
        Ok(())
    }
}

impl<M: Message> MessageRegistration<'_, M> {
    pub(crate) fn add_server_direction(&mut self, direction: NetworkDirection) {
        match direction {
            NetworkDirection::ClientToServer => {
                self.app
                    .register_required_components::<ClientOf, MessageSender<M>>();
            }
            NetworkDirection::ServerToClient => {
                self.app
                    .register_required_components::<ClientOf, MessageReceiver<M>>();
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
                self.app
                    .register_required_components::<ClientOf, TriggerSender<M>>();
            }
            NetworkDirection::Bidirectional => {
                self.add_server_direction(NetworkDirection::ClientToServer);
                self.add_server_direction(NetworkDirection::ServerToClient);
            }
        }
    }
}
