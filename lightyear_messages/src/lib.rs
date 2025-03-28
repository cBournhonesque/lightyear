/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use bevy::ecs::component::ComponentId;
use bevy::prelude::Component;
use lightyear_utils::wrapping_id;

pub(crate) mod registry;
mod plugin;
mod receive;

// TODO: for now messages must be able to be used as events, since we output them in our message events
/// A [`Message`] is basically any type that can be (de)serialized over the network.
///
/// Every type that can be sent over the network must implement this trait.
///
pub trait Message: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Message for T {}

// Internal id that we assign to each message sent over the network
wrapping_id!(MessageId);


/// Component that will track the component_ids of the MessageReceiver<M> and MessageSender<M> that are present on the entity
#[derive(Component)]
#[require(Transport)]
pub struct MessageManager{
    /// List of component ids of the MessageReceiver<M> present on this entity
    pub(crate) receiver_ids: Vec<ComponentId>,
    /// List of component ids of the MessageSender<M> present on this entity
    pub(crate) sender_ids: Vec<ComponentId>,
}