//! NOTE: This does not work anymore since we don't serialize the tick in SingleData anymore!.
use anyhow::Context;

use crate::channel::receivers::fragment_receiver::FragmentReceiver;
use crate::channel::receivers::ChannelReceive;
use crate::packet::message::{MessageContainer, SingleData};
use crate::shared::tick_manager::Tick;
use crate::shared::tick_manager::TickManager;
use crate::shared::time_manager::{TimeManager, WrappedTime};
use crate::utils::ready_buffer::ReadyBuffer;

const DISCARD_AFTER: chrono::Duration = chrono::Duration::milliseconds(3000);

/// Sequenced Unreliable receiver:
/// do not return messages in order, but ignore the messages that are older than the most recent one received
pub struct TickUnreliableReceiver {
    /// Buffer of the messages that we received, but haven't processed yet
    recv_message_buffer: ReadyBuffer<Tick, SingleData>,
    fragment_receiver: FragmentReceiver,
    current_time: WrappedTime,
    current_tick: Tick,
}

// REQUIREMENTS:
// - messages are buffered according to the tick they are associated with
// - at each server tick, we can read the messages that were sent from the corresponding client tick
// - if a message is received too late (its tick is below the current tick, discard + notify client! -> need to speed up client)
// - if a message is received too early (its tick is far above the current tick, buffer it + notify client! -> need to slow down client)
// - TODO: what do we do about sequencing? probably nothing? Should we still order by message-id within a single tick?

impl TickUnreliableReceiver {
    pub fn new() -> Self {
        Self {
            recv_message_buffer: ReadyBuffer::new(),
            fragment_receiver: FragmentReceiver::new(),
            current_time: WrappedTime::default(),
            current_tick: Tick(0),
        }
    }
}

impl TickUnreliableReceiver {
    fn maybe_buffer_data(&mut self, data: SingleData) -> anyhow::Result<()> {
        let tick = data.tick.context("Received a message without tick")?;
        // message is too old
        if tick < self.current_tick {
            // TODO: send message to client to speedup?
        } else {
            // TODO: send message to client to slow down if too far ahead
            self.recv_message_buffer.add_item(tick, data);
        }
        Ok(())
    }
}

impl ChannelReceive for TickUnreliableReceiver {
    fn update(&mut self, time_manager: &TimeManager, tick_manager: &TickManager) {
        self.current_time = time_manager.current_time();
        self.current_tick = tick_manager.tick();
        self.fragment_receiver
            .cleanup(self.current_time - DISCARD_AFTER);
    }

    /// Queues a received message in an internal buffer
    /// The messages are associated with the corresponding tick
    fn buffer_recv(&mut self, message: MessageContainer) -> anyhow::Result<()> {
        match message {
            MessageContainer::Single(data) => self.maybe_buffer_data(data),
            MessageContainer::Fragment(fragment) => {
                if let Some(data) = self
                    .fragment_receiver
                    .receive_fragment(fragment, Some(self.current_time))?
                {
                    return self.maybe_buffer_data(data);
                }
                Ok(())
            }
        }
    }
    fn read_message(&mut self) -> Option<SingleData> {
        self.recv_message_buffer
            .pop_item(&self.current_tick)
            .map(|(_, data)| data)
        // TODO: naia does a more optimized version by return a Vec<Message> instead of Option<Message>
    }
}

#[cfg(test)]
mod tests {
    use bevy::utils::Duration;

    use bytes::Bytes;

    use crate::channel::receivers::ChannelReceive;
    use crate::packet::message::SingleData;
    use crate::shared::tick_manager::TickConfig;

    use super::*;

    #[test]
    fn test_tick_unreliable_receiver_internals() -> anyhow::Result<()> {
        let mut receiver = TickUnreliableReceiver::new();
        let mut tick_manager = TickManager::from_config(TickConfig {
            tick_duration: Duration::from_millis(10),
        });
        let time_manager = TimeManager::new(Duration::default());

        let single1 = SingleData::new(None, Bytes::from("hello"), 1.0);
        let mut single2 = SingleData::new(None, Bytes::from("world"), 1.0);

        // receive a message with no tick -> error
        assert_eq!(
            receiver
                .buffer_recv(single1.clone().into())
                .unwrap_err()
                .to_string(),
            "Received a message without tick",
        );

        // receive an message from an old ticker: it doesn't get added to the buffer
        single2.tick = Some(Tick(60000));
        receiver.buffer_recv(single2.clone().into())?;
        assert_eq!(receiver.recv_message_buffer.len(), 0);

        // receive message for a future tick: it gets added to the buffer
        single2.tick = Some(Tick(2));
        receiver.buffer_recv(single2.clone().into())?;
        assert_eq!(receiver.recv_message_buffer.len(), 1);

        // increment tick by 1: we still haven't reached the tick of the message
        tick_manager.increment_tick();
        receiver.update(&time_manager, &tick_manager);
        assert_eq!(receiver.read_message(), None);

        // increment tick by 1: we can not read the message
        tick_manager.increment_tick();
        receiver.update(&time_manager, &tick_manager);
        assert_eq!(receiver.read_message(), Some(single2.clone()));
        Ok(())
    }
}
