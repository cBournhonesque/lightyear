//! # Lightyear Messages
//!
//! This crate provides a [`MessagePlugin`](crate::plugin::MessagePlugin) to handle sending and receiving messages over the network.
//!
//! A [`Message`] is simply any type that can be (de)serialized.
//!
//! You can add the [`MessageSender<M>`](send::MessageSender) or [`MessageReceiver<M>`](receive::MessageReceiver) components to your Link entity to enable sending and receiving messages of type `M`.
//!
//! The crate also provides a [`MessageManager`] component that manages the process of sending and receiving messages for an entity.
//! It stores a [`RemoteEntityMap`] that holds a mapping between the local and remote entities.

#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

use bevy_ecs::component::{Component, ComponentId};
use bevy_reflect::Reflect;

use crate::registry::MessageKind;
use alloc::vec::Vec;
use lightyear_core::network::NetId;
use lightyear_serde::entity_map::RemoteEntityMap;
use lightyear_transport::prelude::Transport;

#[cfg(feature = "client")]
mod client;
pub mod multi;
pub mod plugin;
pub mod receive;
mod receive_event;
pub mod registry;
pub mod send;
mod send_trigger;
#[cfg(feature = "server")]
pub mod server;
mod trigger;
pub mod prelude {
    pub use crate::plugin::MessageSystems;
    pub use crate::receive::MessageReceiver;
    pub use crate::receive_event::RemoteEvent;
    pub use crate::registry::{AppMessageExt, MessageRegistry};
    pub use crate::send::MessageSender;
    pub use crate::send_trigger::EventSender;
    pub use crate::trigger::AppTriggerExt;
    pub use crate::{Message, MessageManager};

    #[cfg(feature = "server")]
    pub use crate::server::ServerMultiMessageSender;
}

// send-trigger: prepare message TriggerEvent<M> to be sent.
// if TriggerEvent<M> is added, we update `sender_id` with MessageSender<RemoteMessage<M>>.

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
    /// List of component ids of the [`TriggerSender<M>`](send_trigger::EventSender) present on this entity
    pub(crate) send_triggers: Vec<(MessageKind, ComponentId)>,
    /// List of component ids of the [`MessageReceiver<M>`](receive::MessageReceiver) present on this entity
    pub(crate) receive_messages: Vec<(MessageKind, ComponentId)>,
    pub entity_mapper: RemoteEntityMap,
}
