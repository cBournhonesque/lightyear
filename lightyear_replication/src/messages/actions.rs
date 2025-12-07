use super::{change_ranges::ChangeRanges, serialized_data::SerializedData, updates::Updates};
use crate::registry::component_mask::ComponentMask;
use crate::registry::registry::ComponentIndex;
use crate::send::client_pools::ClientPools;
use alloc::vec::Vec;
use bevy_ecs::entity::EntityHashMap;
use bevy_ecs::prelude::*;
use bytes::Bytes;
use core::{iter, mem, ops::Range};
use lightyear_serde::reader::{ReadInteger, Reader};
use lightyear_serde::varint::varint_len;
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};

/// Default channel to replicate entity actions.
/// This is an Sequenced Reliable channel.
/// (SpawnEntity, DespawnEntity, InsertComponent, RemoveComponent)
pub struct ActionsChannel;

/// Entity updates for the current tick.
///
/// The data is serialized manually and stored in the form of ranges
/// from [`SerializedData`].
///
/// Can be packed into a message using [`Self::send`].
#[derive(Component, Default, Debug)]
pub(crate) struct Actions {
    /// Entity mappings for newly visible server entities and their hashes calculated from the [`Signature`] component.
    ///
    /// Spawns should be processed first, so all referenced entities after it will behave correctly.
    spawns: Vec<Range<usize>>,

    /// Number of entity spawns.
    ///
    /// May not be equal to the length of [`Self::mappings`] since adjacent ranges are merged together.
    spawns_len: usize,

    /// Despawns that happened in this tick.
    ///
    /// Since clients may see different entities, it's serialized as multiple chunks of entities.
    /// I.e. serialized server despawns may have holes for clients due to visibility differences.
    despawns: Vec<Range<usize>>,

    /// Number of despawned entities.
    ///
    /// May not be equal to the length of [`Self::despawns`] since adjacent ranges are merged together.
    despawns_len: usize,

    /// Component removals that happened in this tick, for each entity
    removals: EntityHashMap<Vec<ComponentIndex>>,

    /// Component insertions or mutations that happened in this tick.
    ///
    /// Serialized as a list of pairs of entity chunk and multiple chunks with changed components.
    /// Components are stored in multiple chunks because newly connected clients may need to serialize all components,
    /// while previously connected clients only need the components spawned during this tick.
    ///
    /// Usually mutations are stored in [`MutateMessage`], but if an entity has any insertions or removal,
    /// or the entity just became visible for a client, we serialize it as part of the update message to keep entity updates atomic.
    changes: Vec<ChangeRanges>,

    /// Components written in [`Self::changes`].
    changed_components: ComponentMask,

    /// Indicates that an entity has been written since the
    /// last call of [`Self::start_entity_changes`].
    changed_entity_added: bool,
}

impl Actions {
    pub(crate) fn add_mapping(&mut self, mappings: Range<usize>) {
        self.spawns_len += 1;
        if let Some(last) = self.spawns.last_mut() {
            // Append to previous range if possible.
            if last.end == mappings.start {
                last.end = mappings.end;
                return;
            }
        }
        self.spawns.push(mappings);
    }

    pub(crate) fn add_despawn(&mut self, entity: Range<usize>) {
        self.despawns_len += 1;
        if let Some(last) = self.despawns.last_mut() {
            // Append to previous range if possible.
            if last.end == entity.start {
                last.end = entity.end;
                return;
            }
        }
        self.despawns.push(entity);
    }

    /// Adds an entity chunk for removals.
    pub(crate) fn add_removals(
        &mut self,
        pools: &mut ClientPools,
        entity: Entity,
        removed_component: ComponentIndex,
    ) {
        self.removals
            .entry(entity)
            .or_insert_with(|| pools.take_removals())
            .push(removed_component);
    }

    /// Updates internal state to start writing changed components for an entity.
    ///
    /// Entities and their data are written lazily during the iteration.
    /// See [`Self::add_changed_entity`] and [`Self::add_inserted_component`].
    pub(crate) fn start_entity_changes(&mut self) {
        debug_assert!(
            self.changed_components.is_empty(),
            "changed components should be taken before next entity is written"
        );
        self.changed_entity_added = false;
    }

    /// Returns `true` if [`Self::add_changed_entity`] were called since the last
    /// call of [`Self::start_entity_changes`].
    pub(crate) fn changed_entity_added(&mut self) -> bool {
        self.changed_entity_added
    }

    /// Adds an entity chunk for insertions and mutations.
    pub(crate) fn add_changed_entity(&mut self, pools: &mut ClientPools, entity: Range<usize>) {
        self.changes.push(ChangeRanges {
            entity,
            components_len: 0,
            components: pools.take_ranges(),
        });
        self.changed_entity_added = true;
    }

    /// Adds a component chunk to the last added entity from [`Self::add_changed_entity`].
    pub(crate) fn add_inserted_component(&mut self, component: Range<usize>, index: usize) {
        debug_assert!(self.changed_entity_added);
        let changes = self
            .changes
            .last_mut()
            .expect("entity should be written before adding insertions");

        changes.add_component(component);
        self.changed_components.insert(index);
    }

    /// Takes last updated entity with its component chunks from the update message.
    pub(crate) fn take_added_entity(&mut self, pools: &mut ClientPools, updates: &mut Updates) {
        debug_assert!(updates.entity_added());
        let entity_mutations = updates.pop().expect("entity should be written");

        if !self.changed_entity_added {
            self.changes.push(entity_mutations.ranges);
        } else {
            let changes = self.changes.last_mut().expect("entity should be written");
            debug_assert_eq!(entity_mutations.ranges.entity, changes.entity);
            changes.extend(&entity_mutations.ranges);
            pools.recycle_ranges(iter::once(entity_mutations.ranges.components));

            self.changed_components |= &entity_mutations.components;
            pools.recycle_components(entity_mutations.components);
        }
    }

    /// Takes all changed components for the last changed entity that was written.
    pub(crate) fn take_changed_components(&mut self) -> ComponentMask {
        mem::take(&mut self.changed_components)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.changes.is_empty()
            && self.despawns.is_empty()
            && self.removals.is_empty()
            && self.spawns.is_empty()
    }

    fn flags(&self) -> ActionFlags {
        ActionFlags {
            has_spawns: !self.spawns.is_empty(),
            has_despawns: !self.despawns.is_empty(),
            has_removals: !self.removals.is_empty(),
            has_updates: !self.changes.is_empty(),
        }
    }

    /// Clears all chunks.
    ///
    /// Keeps allocated memory for reuse.
    pub(crate) fn clear(&mut self, pools: &mut ClientPools) {
        self.spawns = Default::default();
        self.spawns_len = 0;
        self.despawns.clear();
        self.despawns_len = 0;

        pools.recycle_removals(self.removals.drain().map(|(_, c)| c));
        pools.recycle_ranges(self.changes.drain(..).map(|c| c.components));
    }
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct SpawnAction {
    #[cfg(feature = "prediction")]
    pub(crate) predicted: bool,
    #[cfg(feature = "interpolation")]
    pub(crate) interpolated: bool,
    pub(crate) controlled: bool,
    // If a PreSpawn hash is provided, instead of spawning an entity on the receiver we will try to match
    // with an entity that has the same PreSpawn hash
    pub(crate) prespawn: Option<u64>,
}

impl ToBytes for SpawnAction {
    fn bytes_len(&self) -> usize {
        1 + self.prespawn.map_or(0, |p| 8)
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        #[cfg(all(feature = "prediction", feature = "interpolation"))]
        if self.predicted && self.interpolated {
            return Err(SerializationError::InvalidValue);
        }
        // Use bits to store the flags:
        // 0: predicted
        // 1: interpolated
        // 2: prespawn.is_some()
        // 3: controlled
        let mut flags = 0u8;
        #[cfg(feature = "prediction")]
        if self.predicted {
            flags |= 1 << 0;
        }
        #[cfg(feature = "interpolation")]
        if self.interpolated {
            flags |= 1 << 1;
        }
        if self.prespawn.is_some() {
            flags |= 1 << 2;
        }
        if self.controlled {
            flags |= 1 << 3
        }
        buffer.write_u8(flags)?;
        if let Some(prespawn) = self.prespawn {
            buffer.write_u64(prespawn)?;
        }
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let flags = buffer.read_u8()?;
        #[cfg(feature = "prediction")]
        let predicted = (flags & (1 << 0)) != 0;
        #[cfg(feature = "interpolation")]
        let interpolated = (flags & (1 << 1)) != 0;
        let has_prespawn = (flags & (1 << 2)) != 0;
        let controlled = (flags & (1 << 3)) != 0;

        let prespawn = if has_prespawn {
            Some(buffer.read_u64()?)
        } else {
            None
        };
        Ok(SpawnAction {
            #[cfg(feature = "prediction")]
            predicted,
            #[cfg(feature = "interpolation")]
            interpolated,
            controlled,
            prespawn,
        })
    }
}

pub(crate) struct ActionFlags {
    pub(crate) has_spawns: bool,
    pub(crate) has_despawns: bool,
    pub(crate) has_removals: bool,
    pub(crate) has_updates: bool,
}

#[derive(PartialEq, Debug)]
pub(crate) enum ActionType {
    Spawn,
    Despawn,
    Removal,
    Update,
}

impl ActionFlags {
    // Check which flag is the last one set.
    // This can be used to optimize the message size by not writing the length of the last array.
    pub(crate) fn is_last(&self) -> ActionType {
        if self.has_updates {
            ActionType::Update
        } else if self.has_removals {
            ActionType::Removal
        } else if self.has_despawns {
            ActionType::Despawn
        } else {
            ActionType::Spawn
        }
    }
}

impl ToBytes for ActionFlags {
    fn bytes_len(&self) -> usize {
        1
    }

    fn to_bytes(
        &self,
        buffer: &mut impl WriteInteger,
    ) -> std::result::Result<(), SerializationError> {
        let mut flags = 0u8;
        if self.has_spawns {
            flags |= 1 << 0;
        }
        if self.has_despawns {
            flags |= 1 << 1;
        }
        if self.has_removals {
            flags |= 1 << 2;
        }
        if self.has_updates {
            flags |= 1 << 3;
        }
        buffer.write_u8(flags)?;
        Ok(())
    }

    /// Packs actions into a message.
    ///
    /// Contains tick, mappings, insertions, removals, and despawns that
    /// happened in this tick.
    ///
    /// Sent over [`ServerChannel::Actions`] channel.
    ///
    /// Some data is optional, and their presence is encoded in the [`ActionFlags`] bitset.
    ///
    /// To know how much data array takes, we serialize it's length. We use `usize`,
    /// but we use variable integer encoding, so they are correctly deserialized even
    /// on a client with a different pointer size. However, if the server sends a value
    /// larger than what a client can fit into `usize` (which is very unlikely), the client will panic.
    /// This is expected, as the client can't have an array of such a size anyway.
    ///
    /// Additionally, we don't serialize the size for the last array and
    /// on deserialization just consume all remaining bytes.
    fn from_bytes(buffer: &mut Reader) -> std::result::Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let flags = buffer.read_u8()?;
        let has_spawns = flags & (1 << 0) != 0;
        let has_despawns = flags & (1 << 1) != 0;
        let has_removals = flags & (1 << 2) != 0;
        let has_updates = flags & (1 << 3) != 0;
        Ok(ActionFlags {
            has_spawns,
            has_despawns,
            has_removals,
            has_updates,
        })
    }
}

/// Message containing entity actions
///
/// All entity actions for all entities are sent together in a single message.
pub(crate) struct ActionsMessageSend<'a> {
    pub(crate) actions: &'a Actions,
    // TODO: add tick?
    flags: ActionFlags,
    serialized: &'a SerializedData,
}

impl<'a> ActionsMessageSend<'a> {
    pub(crate) fn new(
        actions: &'a Actions,
        serialized: &'a SerializedData,
    ) -> ActionsMessageSend<'a> {
        ActionsMessageSend {
            actions,
            flags: actions.flags(),
            serialized,
        }
    }
}

impl<'a> ToBytes for ActionsMessageSend<'a> {
    fn bytes_len(&self) -> usize {
        // Precalculate size first to avoid extra allocations.
        let mut message_size = self.flags.bytes_len();
        let is_last = self.flags.is_last();
        if self.flags.has_spawns {
            if is_last != ActionType::Spawn {
                message_size += varint_len(self.actions.spawns_len as u64);
            }
            message_size += self.actions.spawns.iter().map(Range::len).sum::<usize>();
        }
        if self.flags.has_despawns {
            if is_last != ActionType::Despawn {
                message_size += varint_len(self.actions.despawns_len as u64);
            }
            message_size += self.actions.despawns.iter().map(Range::len).sum::<usize>();
        }
        if self.flags.has_removals {
            if is_last != ActionType::Removal {
                message_size += varint_len(self.actions.removals.len() as u64);
            }
            message_size += self
                .actions
                .removals
                .iter()
                .map(|(e, removals)| e.bytes_len() + removals.bytes_len())
                .sum::<usize>();
        }
        if self.flags.has_updates {
            debug_assert_eq!(is_last, ActionType::Update);
            message_size += self
                .actions
                .changes
                .iter()
                .map(ChangeRanges::size)
                .sum::<usize>();
        }
        message_size
    }

    fn to_bytes(
        &self,
        buffer: &mut impl WriteInteger,
    ) -> std::result::Result<(), SerializationError> {
        self.flags.to_bytes(buffer)?;
        let is_last = self.flags.is_last();
        if self.flags.has_spawns {
            if is_last != ActionType::Spawn {
                buffer.write_varint(self.actions.spawns_len as u64)?;
            }
            for range in &self.actions.spawns {
                buffer.write(&self.serialized[range.clone()])?;
            }
        }
        if self.flags.has_despawns {
            if is_last != ActionType::Despawn {
                buffer.write_varint(self.actions.despawns_len as u64)?;
            }
            for range in &self.actions.despawns {
                buffer.write(&self.serialized[range.clone()])?;
            }
        }
        if self.flags.has_removals {
            if is_last != ActionType::Removal {
                buffer.write_varint(self.actions.removals.len() as u64)?;
            }
            for (entity, removals) in &self.actions.removals {
                entity.to_bytes(buffer)?;
                buffer.write_varint(removals.len() as u64)?;
                for fns_id in removals {
                    fns_id.to_bytes(buffer)?;
                }
            }
        }
        if self.flags.has_updates {
            // Changes are always last, don't write len for it.
            for changes in &self.actions.changes {
                buffer.write(&self.serialized[changes.entity])?;
                buffer.write_varint(changes.components_len as u64)?;
                for component in &changes.components {
                    buffer.write(&self.serialized[component.clone()])?;
                }
            }
        }
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> std::result::Result<Self, SerializationError>
    where
        Self: Sized,
    {
        unreachable!()
    }
}

pub(crate) struct ActionsMessage(pub(crate) Bytes);

impl ToBytes for ActionsMessage {
    fn bytes_len(&self) -> usize {
        self.0.len()
    }

    fn to_bytes(
        &self,
        buffer: &mut impl WriteInteger,
    ) -> std::result::Result<(), SerializationError> {
        buffer.write(&self.0)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> std::result::Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(ActionsMessage(Bytes::from_bytes(buffer)?))
    }
}
