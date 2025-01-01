//! This module defines bevy [`Events`](bevy::prelude::Events) related to networking events

use crate::prelude::{Channel, ChannelKind, Message, NetworkTarget};
use bevy::prelude::{Event, Resource};
use std::error::Error;

pub mod components;
pub(crate) mod connection;
pub mod plugin;
pub mod systems;

/// Shared trait between client and server to send messages to a target
pub trait EventSend: private::InternalEventSend {
    /// Replicate the event to the target via a channel and then write to an EventWriter in the
    /// remote World
    fn send_event_to_target<C: Channel, E: Event + Message>(
        &mut self,
        event: &E,
        target: NetworkTarget,
    ) -> Result<(), Self::Error> {
        self.erased_send_event_to_target(event, ChannelKind::of::<C>(), target)
    }

    /// Replicate the `event` to the `target` via channel [`C`] and then trigger the event
    /// in the remote World
    fn trigger_event_to_target<C: Channel, E: Event + Message>(
        &mut self,
        event: &E,
        target: NetworkTarget,
    ) -> Result<(), Self::Error> {
        self.erased_trigger_event_to_target(event, ChannelKind::of::<C>(), target)
    }
}

pub(crate) mod private {
    use super::*;

    pub trait InternalEventSend: Resource {
        type Error: Error;
        fn erased_send_event_to_target<E: Event>(
            &mut self,
            event: &E,
            channel_kind: ChannelKind,
            target: NetworkTarget,
        ) -> Result<(), Self::Error>;

        fn erased_trigger_event_to_target<E: Event + Message>(
            &mut self,
            event: &E,
            channel_kind: ChannelKind,
            target: NetworkTarget,
        ) -> Result<(), Self::Error>;
    }
}
