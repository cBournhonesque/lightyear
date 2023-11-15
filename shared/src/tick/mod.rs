use bevy::prelude::Resource;

use lightyear_derive::ChannelInternal;

use crate::utils::wrapping_id;

pub(crate) mod manager;
pub(crate) mod message;
pub(crate) mod ping_store;
pub(crate) mod time;

/// Internal id that tracks the Tick value for the server and the client
wrapping_id!(Tick);

pub trait TickManaged: Resource {
    fn increment_tick(&mut self);
}

/// Channel where the messages are buffered according to the tick they are associated with
/// At each server tick, we can read the messages that were sent from the corresponding client tick
#[derive(ChannelInternal)]
pub struct TickBufferChannel;
