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

pub mod plugin;
pub mod receive;
pub mod send;
mod trigger;
pub mod registry;
mod send_trigger;
mod receive_trigger;
#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod server;

pub mod prelude {
    pub use crate::receive::MessageReceiver;
    pub use crate::receive_trigger::RemoteTrigger;
    pub use crate::registry::AppMessageExt;
    pub use crate::send::MessageSender;
    pub use crate::send_trigger::TriggerSender;
    pub use crate::trigger::AppTriggerExt;
    pub use crate::{Message, MessageManager};
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


/// Component that will track the component_ids of the MessageReceiver<M> and MessageSender<M> that are present on the entity
#[derive(Component, Default, Reflect)]
#[require(Transport)]
pub struct MessageManager{
    /// List of component ids of the MessageSender<M> present on this entity
    pub(crate) send_messages: Vec<(MessageKind, ComponentId)>,
    /// List of component ids of the TriggerSender<M> present on this entity
    pub(crate) send_triggers: Vec<(MessageKind, ComponentId)>,
    pub entity_mapper: RemoteEntityMap,
}