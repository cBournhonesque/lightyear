use crate::prelude::{AppMessageExt, MessageRegistry};
use crate::receive::{MessageReceiver, ReceivedMessage, push_received_message_for_test};
use crate::send::{MessageSender, drain_buffered_messages_for_test};
use bevy_app::App;
use core::hint::black_box;
use lightyear_core::tick::Tick;
use lightyear_serde::entity_map::{ReceiveEntityMap, SendEntityMap};
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::writer::{WriteInteger, Writer};
use lightyear_serde::{SerializationError, ToBytes};

/// Small fixed-size message used by allocation regression fixtures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllocationTestMessage {
    sequence: u32,
    payload: u64,
}

impl AllocationTestMessage {
    fn new(sequence: u32) -> Self {
        Self {
            sequence,
            payload: (sequence as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
        }
    }

    fn checksum(self) -> u64 {
        self.sequence as u64 ^ self.payload
    }
}

impl ToBytes for AllocationTestMessage {
    fn bytes_len(&self) -> usize {
        12
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        buffer.write_u32(self.sequence)?;
        buffer.write_u64(self.payload)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError> {
        Ok(Self {
            sequence: buffer.read_u32()?,
            payload: buffer.read_u64()?,
        })
    }
}

struct AllocationTestChannel;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MessageLoopStats {
    pub messages: usize,
    pub checksum: u64,
}

#[derive(Default)]
pub struct MessageQueueFixture {
    sender: MessageSender<AllocationTestMessage>,
    receiver: MessageReceiver<AllocationTestMessage>,
}

impl MessageQueueFixture {
    pub fn run_messages(
        &mut self,
        message_count: usize,
        messages_per_batch: usize,
    ) -> MessageLoopStats {
        let mut stats = MessageLoopStats::default();
        let mut sequence = 0;

        while sequence < message_count {
            let batch_end = (sequence + messages_per_batch).min(message_count);
            for i in sequence..batch_end {
                self.sender
                    .send::<AllocationTestChannel>(AllocationTestMessage::new(i as u32));
            }

            for (message, channel_kind, _, _) in drain_buffered_messages_for_test(&mut self.sender)
            {
                push_received_message_for_test(
                    &mut self.receiver,
                    ReceivedMessage {
                        data: message,
                        remote_tick: Tick(sequence as u32),
                        channel_kind,
                        message_id: None,
                    },
                );
            }

            for received in self.receiver.receive_with_tick() {
                stats.messages += 1;
                stats.checksum ^= received.data.checksum();
                black_box(received);
            }

            sequence = batch_end;
        }

        stats
    }
}

pub struct MessageSerializationFixture {
    registry: MessageRegistry,
    writer: Writer,
    send_entity_map: SendEntityMap,
    receive_entity_map: ReceiveEntityMap,
}

impl Default for MessageSerializationFixture {
    fn default() -> Self {
        let mut app = App::new();
        app.register_message_to_bytes::<AllocationTestMessage>();
        let registry = app.world().resource::<MessageRegistry>().clone();
        Self {
            registry,
            writer: Writer::default(),
            send_entity_map: SendEntityMap::default(),
            receive_entity_map: ReceiveEntityMap::default(),
        }
    }
}

impl MessageSerializationFixture {
    pub fn run_messages(
        &mut self,
        message_count: usize,
    ) -> Result<MessageLoopStats, crate::registry::MessageError> {
        let mut stats = MessageLoopStats::default();

        for i in 0..message_count {
            let message = AllocationTestMessage::new(i as u32);
            self.registry
                .serialize(&message, &mut self.writer, &mut self.send_entity_map)?;
            let bytes = self.writer.split();
            let mut reader = Reader::from(bytes);
            let decoded: AllocationTestMessage = self
                .registry
                .deserialize(&mut reader, &mut self.receive_entity_map)?;

            stats.messages += 1;
            stats.checksum ^= decoded.checksum();
            black_box(decoded);
        }

        Ok(stats)
    }
}
