//! # Lightyear Messages
//!
//! This crate provides the system for sending and receiving messages over the network.
//!
//! It defines the `Message` trait, which all messages must implement, and provides
//! utilities for managing message sending and receiving, including `MessageSender`,
//! `MessageReceiver`, and `MessageManager`.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::registry::MessageKind;
use bevy::ecs::component::ComponentId;
use bevy::prelude::{Component, Reflect};
use lightyear_core::network::NetId;
use lightyear_serde::entity_map::RemoteEntityMap;
use lightyear_transport::prelude::Transport;

#[cfg(feature = "client")]
mod client;
pub mod multi;
pub mod plugin;
pub mod receive;
mod receive_trigger;
pub mod registry;
pub mod send;
mod send_trigger;
#[cfg(feature = "server")]
pub mod server;
mod trigger;
pub mod prelude {
    pub use crate::plugin::MessageSet;
    pub use crate::receive::MessageReceiver;
    pub use crate::receive_trigger::RemoteTrigger;
    pub use crate::registry::AppMessageExt;
    pub use crate::send::MessageSender;
    pub use crate::send_trigger::TriggerSender;
    pub use crate::trigger::AppTriggerExt;
    pub use crate::{Message, MessageManager};

    #[cfg(feature = "server")]
    pub use crate::server::ServerMultiMessageSender;
}

// send-trigger: prepare message TriggerMessage<M> to be sent.
// if TriggerMessage<M> is added, we update `sender_id` with MessageSender<RemoteMessage<M>>.

// TODO: for now messages must be able to be used as events, since we output them in our message events
/// A [`Message`] is basically any type that can be (de)serialized over the network.
///
/// Every type that can be sent over the network must implement this trait.
///
pub trait Message: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Message for T {}

// // Internal id that we assign to each message sent over the network
// wrapping_id!(MessageId);
// TODO: this conflicts with the MessageId from lightyear_transport! find a different name
pub type MessageNetId = NetId;

/// Manages sending and receiving messages for an entity.
///
/// This component is added to entities that need to send or receive messages.
/// It keeps track of the [`MessageSender<M>`](send::MessageSender) and [`MessageReceiver<M>`](receive::MessageReceiver) components
/// attached to the entity, allowing the messaging system to interact with them.
/// It also holds a [`RemoteEntityMap`] for mapping entities between client and server.
#[derive(Component, Default, Reflect)]
#[require(Transport)]
pub struct MessageManager {
    /// List of component ids of the [`MessageSender<M>`](send::MessageSender) present on this entity
    pub(crate) send_messages: Vec<(MessageKind, ComponentId)>,
    /// List of component ids of the [`TriggerSender<M>`](send_trigger::TriggerSender) present on this entity
    pub(crate) send_triggers: Vec<(MessageKind, ComponentId)>,
    /// List of component ids of the [`MessageReceiver<M>`](receive::MessageReceiver) present on this entity
    pub(crate) receive_messages: Vec<(MessageKind, ComponentId)>,
    pub entity_mapper: RemoteEntityMap,
}
