use super::{change_ranges::ChangeRanges, serialized_data::SerializedData};
use crate::error::ReplicationError;
use crate::registry::component_mask::ComponentMask;
use crate::send::client_pools::ClientPools;
use crate::send::sender_ticks::{SenderTicks, UpdateInfo};
use alloc::vec;
use alloc::vec::Vec;
use bevy_ecs::component::Tick as BevyTick;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::*;
use bytes::Bytes;
use core::{iter, mem, ops::Range};
use lightyear_core::prelude::Tick;
use lightyear_serde::reader::Reader;
use lightyear_serde::writer::{WriteInteger, Writer};
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_transport::prelude::Transport;
use std::io::Write;

/// Default channel to replicate entity updates (ComponentUpdate)
/// This is a Sequenced Unreliable channel
pub struct UpdatesChannel;

/// Component mutations for the current tick.
///
/// The data is serialized manually and stored in the form of ranges
/// from [`SerializedData`].
///
/// Can be packed into messages using [`Self::send`].
#[derive(Component, Default)]
pub(crate) struct Updates {
    /// Entities that are related to each other and should be replicated in sync.
    ///
    /// Like [`Self::standalone`], but grouped into arrays based on their relation graph indices.
    /// These entities are guaranteed to be included in a single message.
    related: Vec<Vec<EntityUpdates>>,

    /// Component mutations that happened in this tick.
    ///
    /// These mutation are not related to any others and can be replicated independently.
    standalone: Vec<EntityUpdates>,

    /// Location of the last written entity since the last call of [`Self::start_entity_mutations`].
    entity_location: Option<EntityLocation>,
}

impl Updates {
    /// Updates internal state to start writing mutated components for an entity.
    ///
    /// Entities and their data written lazily during the iteration.
    /// See [`Self::add_entity`] and [`Self::add_component`].
    pub(crate) fn start_entity(&mut self) {
        self.entity_location = None;
    }

    /// Returns `true` if [`Self::add_entity`] were called since the last
    /// call of [`Self::start_entity`].
    pub(crate) fn entity_added(&mut self) -> bool {
        self.entity_location.is_some()
    }

    /// Adds an entity chunk.
    pub(crate) fn add_entity(
        &mut self,
        pools: &mut ClientPools,
        entity: Entity,
        is_in_last_group: bool,
        is_new_group: bool,
        entity_range: Range<usize>,
    ) {
        let mutations = EntityUpdates {
            entity,
            ranges: ChangeRanges {
                entity: entity_range,
                components_len: 0,
                components: pools.take_ranges(),
            },
            components: pools.take_components(),
        };

        if is_new_group {
            // TODO: how to avoid this allocation?
            self.related.push(vec![mutations]);
            self.entity_location = Some(EntityLocation::Related { index });
        } else if is_in_last_group {
            let index = self.related.len() - 1;
            self.related[index].push(mutations);
            self.entity_location = Some(EntityLocation::Related { index });
        } else {
            self.entity_location = Some(EntityLocation::Standalone);
            self.standalone.push(mutations);
        }
    }

    /// Adds a component chunk to the last added entity from [`Self::add_entity`].
    pub(crate) fn add_component(&mut self, component: Range<usize>) {
        let mutations = self
            .entity_location
            .and_then(|location| match location {
                EntityLocation::Related { index } => self.related[index].last_mut(),
                EntityLocation::Standalone => self.standalone.last_mut(),
            })
            .expect("entity should be written before adding components");

        mutations.ranges.add_component(component);
    }

    /// Removes last added entity from [`Self::add_entity`] and returns it.
    pub(super) fn pop(&mut self) -> Option<EntityUpdates> {
        self.entity_location
            .take()
            .and_then(|location| match location {
                EntityLocation::Related { index } => self.related[index].pop(),
                EntityLocation::Standalone => self.standalone.pop(),
            })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.standalone.is_empty() && self.related.is_empty()
    }

    /// Packs mutations into messages.
    ///
    /// Contains update tick, current tick, mutate index and component mutations since
    /// the last acknowledged tick for each entity.
    ///
    /// Cannot be applied on the client until the update message matching this message's update tick
    /// has been applied to the client world.
    /// The message will be manually split into packets up to max size, and each packet will be applied
    /// independently on the client.
    /// Message splits only happen per-entity to avoid weird behavior from partial entity mutations.
    ///
    /// Sent over the [`ServerChannel::Mutations`] channel. If the message gets lost, we try to resend it manually,
    /// using the last up-to-date mutations to avoid re-sending old values.
    pub(crate) fn send(
        &mut self,
        writer: &mut Writer,
        transport: &mut Transport,
        sender_ticks: &mut SenderTicks,
        pools: &mut ClientPools,
        serialized: &SerializedData,
        tick: Tick,
        system_tick: BevyTick,
    ) -> Result<(), ReplicationError> {
        // TODO: change this?
        debug_assert!(writer.is_empty());
        // estimated header size
        // 11 for packet-header, 2 for message-id, 2 for channel-id, 5 for safety
        const HEADER_SIZE: usize = 20;

        // max size for the contents of a message
        const MAX_SIZE: usize = 1200;

        // TODO: we need last_action_tick to be an option because of wrapping
        sender_ticks.action_tick.to_bytes(writer)?;
        // We include the tick even though the tick is sent during messages in case the
        // message is not sent immediately because of priority
        tick.to_bytes(writer)?;
        let metadata_size = writer.len();

        let mut update_info = UpdateInfo {
            server_tick,
            system_tick,
            entities: pools.take_entities(),
        };
        let mut chunks = EntityChunks::new(&mut self.related, &mut self.standalone);
        let mut header_size = metadata_size + HEADER_SIZE;
        let mut body_size = 0;
        let mut chunks_range = Range::<usize>::default();
        for chunk in chunks.iter_mut() {
            let mut mutations_size = 0;
            for mutations in &mut *chunk {
                mutations_size += mutations.ranges.size_with_components_size();
            }

            // Try to pack back first, then try to pack forward.
            if body_size != 0
                && !can_pack(header_size + body_size, mutations_size, MAX_SIZE)
                && !can_pack(header_size + mutations_size, body_size, MAX_SIZE)
            {
                // message is full! send it
                let message_bytes = writer.split();
                let message_id = transport
                    .send_mut_with_priority::<UpdatesChannel>(message_bytes, priority)?
                    .expect("The entity updates channels should always return a message_id");
                sender_ticks.register_update_message(message_id, update_info);

                update_info = UpdateInfo {
                    server_tick,
                    system_tick,
                    entities: pools.take_entities(),
                };
                // start a new message
                sender_ticks.action_tick.to_bytes(writer)?;
                tick.to_bytes(writer)?;
                chunks_range.start = chunks_range.end;
                body_size = 0;
            }

            update_info
                .entities
                .extend(chunk.iter_mut().map(|entity_updates| {
                    (
                        entity_updates.entity,
                        mem::take(&mut entity_updates.components),
                    )
                }));

            // write the contents of the chunk
            for updates in chunk {
                writer.write(&serialized[updates.ranges.entity])?;
                writer.write_varint(updates.ranges.components_size() as u64)?;
                for component in updates.ranges.components {
                    writer.write(&serialized[component])?;
                }
            }
            chunks_range.end += 1;
            body_size += mutations_size;
        }
        if !chunks_range.is_empty() {
            // When the loop ends, pack all leftovers into a message.
            // Or create an empty message if tracking mutate messages is enabled.
            for updates in chunks.iter_flatten(chunks_range) {
                writer.write(&serialized[updates.ranges.entity])?;
                writer.write_varint(updates.ranges.components_size() as u64)?;
                for component in updates.ranges.components {
                    writer.write(&serialized[component])?;
                }
            }
            let message_bytes = writer.split();
            let message_id = transport
                .send_mut_with_priority::<UpdatesChannel>(message_bytes, priority)?
                .expect("The entity updates channels should always return a message_id");
            sender_ticks.register_update_message(message_id, update_info);
        }
        Ok(())
    }

    /// Clears all entity mutations.
    ///
    /// Keeps allocated memory for reuse.
    pub(crate) fn clear(&mut self, pools: &mut ClientPools) {
        for entities in self
            .related
            .iter_mut()
            .chain(iter::once(&mut self.standalone))
        {
            pools.recycle_ranges(entities.drain(..).map(|m| m.ranges.components));
            // We don't take component masks because they are moved to `MutateInfo` during sending.
        }
        pools.recycle_mutations(self.related.drain(..))
    }
}

/// Update data for [`Updates::related`] and [`Updates::standalone`].
pub(crate) struct EntityUpdates {
    /// Associated entity.
    ///
    /// Used to associate entities with the update message index that the client
    /// needs to acknowledge to consider entity updates received.
    entity: Entity,

    /// Component updates that happened in this tick.
    ///
    /// Serialized as a list of pairs of entity chunk and multiple chunks with updated components.
    /// Components are stored in multiple chunks because some clients may acknowledge updates,
    /// while others may not.
    ///
    /// Unlike with [`Actions`](super::actions::Actions), we serialize the number
    /// of chunk bytes instead of the number of components. This is because, during deserialization,
    /// some entities may be skipped if they have already been updated (as updates are sent until
    /// the client acknowledges them).
    pub(super) ranges: ChangeRanges,

    /// Components written in [`Self::ranges`].
    ///
    /// Like [`Self::entity`], used for later component acknowledgement.
    pub(super) components: ComponentMask,
}

#[derive(Clone, Copy)]
enum EntityLocation {
    Related { index: usize },
    Standalone,
}

/// Treats related and standalone entity mutations as a single continuous buffer,
/// with related entities first, followed by standalone ones.
struct EntityChunks<'a> {
    related: &'a mut [Vec<EntityUpdates>],
    standalone: &'a mut [EntityUpdates],
}

impl<'a> EntityChunks<'a> {
    fn new(related: &'a mut [Vec<EntityUpdates>], standalone: &'a mut [EntityUpdates]) -> Self {
        Self {
            related,
            standalone,
        }
    }

    /// Returns an iterator over slices of related entities.
    ///
    /// Standalone entities are represented as single-element slices.
    fn iter_mut(&mut self) -> impl Iterator<Item = &mut [EntityUpdates]> {
        self.related
            .iter_mut()
            .map(Vec::as_mut_slice)
            .chain(self.standalone.chunks_mut(1))
    }

    /// Returns an iterator over flattened slices of entity mutations within the specified range.
    ///
    /// The range indexes chunk numbers (not individual elements).
    fn iter_flatten(&self, range: Range<usize>) -> impl Iterator<Item = &EntityUpdates> {
        let total_len = self.related.len() + self.standalone.len();
        debug_assert!(range.start <= total_len);
        debug_assert!(range.end <= total_len);

        let split_point = self.related.len();

        let related_start = range.start.min(split_point);
        let related_end = range.end.min(split_point);
        let standalone_start = range.start.saturating_sub(split_point);
        let standalone_end = range.end.saturating_sub(split_point);

        let related_range = related_start..related_end;
        let standalone_range = standalone_start..standalone_end;

        self.related[related_range]
            .iter()
            .flatten()
            .chain(&self.standalone[standalone_range])
    }
}

/// Information about updates that are split into a message.
///
/// We split mutations into messages first in order to know their count in advance.
pub(crate) struct UpdatesSplit {
    message_size: usize,
    /// Indices in [`EntityChunks`].
    chunks_range: Range<usize>,
}

/// Returns `true` if the additional data fits within the remaining space
/// of the current packet tail.
///
/// When the message already exceeds the MTU, more data can be packed
/// as long as it fits within the last partial packet without causing
/// an additional packet to be created.
fn can_pack(message_size: usize, add: usize, mtu: usize) -> bool {
    let dangling = message_size % mtu;
    (dangling > 0) && ((dangling + add) <= mtu)
}

#[derive(Debug, PartialEq)]
pub(crate) struct UpdatesMessage {
    pub(crate) remote_tick: Tick,
    pub(crate) last_action_tick: Option<Tick>,
    pub(crate) data: Bytes,
}

impl ToBytes for UpdatesMessage {
    fn bytes_len(&self) -> usize {
        unreachable!()
    }

    fn to_bytes(
        &self,
        buffer: &mut impl WriteInteger,
    ) -> std::result::Result<(), SerializationError> {
        unreachable!()
    }

    fn from_bytes(buffer: &mut Reader) -> std::result::Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let remote_tick = Tick::from_bytes(buffer)?;
        let last_action_tick = Tick::from_bytes(buffer)?;
        // as an optimization, to serialize 'no last action tick constraint'
        // we use `remote_tick = last_action_tick`. This case is never used
        // otherwise since this is an Updates message and not an Actions message.
        let last_action_tick = if last_action_tick == remote_tick {
            Some(last_action_tick)
        } else {
            None
        };
        Ok(Self {
            remote_tick,
            last_action_tick,
            data: buffer.split(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_SIZE: usize = 1200;

    #[test]
    fn packing() {
        assert!(can_pack(10, 5, MAX_SIZE));
        assert!(can_pack(10, 1190, MAX_SIZE));
        assert!(!can_pack(10, 1191, MAX_SIZE));
        assert!(!can_pack(10, 3000, MAX_SIZE));

        assert!(can_pack(1500, 500, MAX_SIZE));
        assert!(can_pack(1500, 900, MAX_SIZE));
        assert!(!can_pack(1500, 1000, MAX_SIZE));

        assert!(can_pack(1199, 1, MAX_SIZE));
        assert!(!can_pack(1200, 0, MAX_SIZE));
        assert!(!can_pack(1200, 1, MAX_SIZE));
        assert!(!can_pack(1200, 3000, MAX_SIZE));
    }

    #[test]
    fn splitting() {
        assert_eq!(send([], [], false), 0);
        assert_eq!(send([], [10], false), 1);
        assert_eq!(send([], [1300], false), 1);
        assert_eq!(send([], [20, 20], false), 1);
        assert_eq!(send([], [700, 700], false), 2);
        assert_eq!(send([], [1300, 700], false), 1);
        assert_eq!(send([], [1300, 1300], false), 2);

        assert_eq!(send([&[10]], [], false), 1);
        assert_eq!(send([&[1300]], [], false), 1);
        assert_eq!(send([&[20, 20]], [], false), 1);
        assert_eq!(send([&[700, 700]], [], false), 1);
        assert_eq!(send([&[1300, 1300]], [], false), 1);
        assert_eq!(send([&[20], &[20]], [], false), 1);
        assert_eq!(send([&[700], &[700]], [], false), 2);
        assert_eq!(send([&[1300], &[1300]], [], false), 2);

        assert_eq!(send([&[10]], [10], false), 1);
        assert_eq!(send([&[1300]], [1300], false), 2);
        assert_eq!(send([&[20, 20]], [20, 20], false), 1);
        assert_eq!(send([&[700, 700]], [700, 700], false), 2);
        assert_eq!(send([&[1300, 1300]], [1300, 1300], false), 3);
        assert_eq!(send([&[20], &[20]], [20], false), 1);
        assert_eq!(send([&[700], &[700]], [700], false), 3);
        assert_eq!(send([&[1300], &[1300]], [1300], false), 3);

        assert_eq!(send([], [], true), 1);
        assert_eq!(send([], [10], true), 1);
        assert_eq!(send([&[10]], [], true), 1);
        assert_eq!(send([&[10]], [10], true), 1);
        assert_eq!(send([], [1194], true), 1);
    }

    /// Mocks message sending with specified data sizes.
    ///
    /// `related` and `standalone` specify sizes for entities and their mutations.
    /// See also [`write_entity`].
    fn send<const N: usize, const M: usize>(
        related: [&[usize]; N],
        standalone: [usize; M],
        track_mutate_messages: bool,
    ) -> usize {
        let mut serialized = SerializedData::default();
        let mut messages = ServerMessages::default();
        let mut mutations = Updates::default();
        let mut pools = ClientPools::default();

        mutations.resize_related(&mut pools, related.len());

        for (index, &entities) in related.iter().enumerate() {
            for &mutations_size in entities {
                write_entity(
                    &mut mutations,
                    &mut serialized,
                    &mut pools,
                    Some(index),
                    mutations_size,
                );
            }
        }

        for &mutations_size in &standalone {
            write_entity(
                &mut mutations,
                &mut serialized,
                &mut pools,
                None,
                mutations_size,
            );
        }

        mutations
            .send(
                &mut messages,
                Entity::PLACEHOLDER,
                &mut Default::default(),
                &mut Default::default(),
                &mut Default::default(),
                &serialized,
                track_mutate_messages,
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                MAX_SIZE,
            )
            .unwrap()
    }

    /// Mocks writing an entity with a single mutated component of specified size.
    ///
    /// 4 bytes will be used for the entity, with the remaining space used by the component.
    /// All written data will be zeros.
    fn write_entity(
        mutations: &mut Updates,
        serialized: &mut SerializedData,
        pools: &mut ClientPools,
        graph_index: Option<usize>,
        mutations_size: usize,
    ) {
        assert!(mutations_size > 4);
        let start = serialized.len();
        serialized.resize(start + mutations_size, 0);

        let entity_size = start + 4;
        mutations.start_entity();
        mutations.add_entity(pools, Entity::PLACEHOLDER, graph_index, start..entity_size);
        mutations.add_component(entity_size..serialized.len());
    }
}
