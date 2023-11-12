use crate::tick::Tick;
use crate::ReadyBuffer;
use bevy::prelude::Resource;

// TODO: should we request that a user input is a message?
pub trait UserInput: Clone + Eq + PartialEq + Send + Sync + 'static {}

#[derive(Resource)]
pub struct InputBuffer<T: UserInput> {
    buffer: ReadyBuffer<Tick, T>,
}

impl<T: UserInput> Default for InputBuffer<T> {
    fn default() -> Self {
        Self {
            buffer: ReadyBuffer::new(),
        }
    }
}

impl<T: UserInput> InputBuffer<T> {
    /// Add an input that the user sent at a specific tick.
    pub fn insert(&mut self, tick: Tick, input: T) {
        // TODO: error if we send for a tick that is not the current tick?
        // TODO: maybe just paass the tick_manager as input?
        self.buffer.add_item(tick, input);
    }

    /// We have copied the server_state into the client_state
    /// Now provide the client_state with the inputs that the user sent in start_tick..=current_tick
    pub fn replay(&mut self, start_tick: Tick) {}
}
