use crate::Message;
use crate::multi::MultiMessageSender;
use crate::prelude::{MessageReceiver, MessageSender};
use crate::registry::MessageRegistration;
use crate::send::Priority;
use crate::send_trigger::EventSender;
use crate::trigger::TriggerRegistration;
use bevy_ecs::entity::EntitySet;
use bevy_ecs::query::QueryFilter;
use bevy_ecs::{
    error::Result,
    event::Event,
    relationship::RelationshipTarget,
    system::{Res, SystemParam},
};
use lightyear_connection::client::PeerMetadata;
use lightyear_connection::client_of::ClientOf;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::network_target::NetworkTarget;
use lightyear_link::prelude::Server;
use lightyear_transport::channel::Channel;

/// SystemParam to help send a message to the different [`ClientOf`] connected to a [`Server`].
///
/// Wrapper around a [`MultiMessageSender`] to allow sending messages to clients
/// by referring to them using their [`PeerId`](lightyear_core::prelude::PeerId)
/// instead of their Entity.
#[derive(SystemParam)]
pub struct ServerMultiMessageSender<'w, 's, F: QueryFilter + 'static = ()> {
    sender: MultiMessageSender<'w, 's, F>,
    metadata: Res<'w, PeerMetadata>,
}

impl<'w, 's, F: QueryFilter> ServerMultiMessageSender<'w, 's, F> {
    /// Send a message to the [`ClientOf`]s matching the [`NetworkTarget`] for the provided [`Server`]
    pub fn send<M: Message, C: Channel>(
        &mut self,
        message: &M,
        server: &Server,
        target: &NetworkTarget,
    ) -> Result {
        self.send_with_priority::<M, C>(message, server, target, 1.0)
    }

    /// Send a message to the [`ClientOf`]s matching the [`NetworkTarget`] for the provided [`Server`]
    ///
    /// Specifies a priority which is used if bandwidth limiting is used.
    pub fn send_with_priority<M: Message, C: Channel>(
        &mut self,
        message: &M,
        server: &Server,
        target: &NetworkTarget,
        priority: Priority,
    ) -> Result {
        // Resolve the NetworkTarget to concrete entities, then delegate to
        // MultiMessageSender which handles per-client entity mapping correctly.
        let mut targets = bevy_ecs::entity::EntityHashSet::default();
        target.apply_targets(
            server.collection().iter().copied(),
            &self.metadata.mapping,
            &mut |entity| {
                targets.insert(entity);
            },
        );
        self.sender
            .send_with_priority::<M, C>(message, &targets, priority)
    }

    /// Send a message to a set of  [`ClientOf`]s entities associated with the provided [`Server`]
    pub fn send_to_entities<M: Message, C: Channel>(
        &mut self,
        message: &M,
        target: impl EntitySet,
    ) -> Result {
        self.send_to_entities_with_priority::<M, C>(message, target, 1.0)
    }

    /// Send a message to a set of  [`ClientOf`]s entities associated with the provided [`Server`]
    ///
    /// Specifies a priority which is used if bandwidth limiting is used.
    pub fn send_to_entities_with_priority<M: Message, C: Channel>(
        &mut self,
        message: &M,
        target: impl EntitySet,
        priority: Priority,
    ) -> Result {
        self.sender
            .send_with_priority::<M, C>(message, target, priority)
    }
}

impl<M: Message> MessageRegistration<'_, M> {
    pub(crate) fn add_server_direction(&mut self, direction: NetworkDirection) {
        match direction {
            NetworkDirection::ClientToServer => {
                self.app
                    .register_required_components::<ClientOf, MessageReceiver<M>>();
            }
            NetworkDirection::ServerToClient => {
                self.app
                    .register_required_components::<ClientOf, MessageSender<M>>();
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
            NetworkDirection::ClientToServer => {
                // empty because we only have a Sender component, not a Receiver component
            }
            NetworkDirection::ServerToClient => {
                self.app
                    .register_required_components::<ClientOf, EventSender<M>>();
            }
            NetworkDirection::Bidirectional => {
                self.add_server_direction(NetworkDirection::ClientToServer);
                self.add_server_direction(NetworkDirection::ServerToClient);
            }
        }
    }
}
