//! Logic related to delta compression (sending only the changes between two states, instead of the new state)

use crate::prelude::{ComponentRegistry, Message, Tick};
use crate::protocol::component::ComponentKind;
use crate::shared::replication::components::ReplicationGroupId;
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::EntityHash;
use bevy::prelude::Entity;
use bevy::ptr::Ptr;
use bevy::utils::HashMap;
use bitcode::{Decode, Encode};
use hashbrown::hash_map::Entry;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::collections::BTreeMap;
use std::ptr::NonNull;

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum DeltaType {
    /// This delta is computed from a previous value
    Normal,
    /// This delta is computed from the Base value
    FromBase,
}

/// A message that contains a delta between two states (for serializing delta compression)
// Need repr(C) to be able to cast the pointer to a u8 pointer
#[repr(C)]
#[derive(Encode, Decode, Deserialize, Serialize)]
pub struct DeltaMessage<M> {
    pub(crate) delta_type: DeltaType,
    #[bitcode(with_serde)]
    pub(crate) delta: M,
}

/// A type is Diffable when you can:
/// - Compute the delta between two states
/// - Apply the delta to an old state to get the new state
pub trait Diffable: Clone {
    /// The type of the delta between two states
    type Delta: Message;

    /// For the first message (when there is no diff possible), instead of sending the full state
    /// we can compute a delta compared to the `Base` default state
    fn base_value() -> Self;

    /// Compute the diff from the old state (self) to the new state (new)
    fn diff(&self, new: &Self) -> Self::Delta;

    /// Apply a delta to the current state to reach the new state
    fn apply_diff(&mut self, delta: &Self::Delta);
}

#[derive(Default)]
pub struct DeltaManager {
    pub(crate) data: DeltaComponentStore,
    pub(crate) acks: EntityHashMap<ReplicationGroupId, HashMap<Tick, usize>>,
}

impl DeltaManager {
    /// We receive an ack from a client for a specific tick.
    /// Update the ack information;
    pub(crate) fn receive_ack(
        &mut self,
        tick: Tick,
        replication_group: ReplicationGroupId,
        component_registry: &ComponentRegistry,
    ) {
        let mut delete = false;
        if let Some(sent_number) = self
            .acks
            .get_mut(&replication_group)
            .unwrap()
            .get_mut(&tick)
        {
            if *sent_number == 1 {
                // TODO: maybe optimize this by keeping track in each message of which delta compression components were included?
                // all the clients have acked this message, we can remove the data for all ticks older than this one
                delete = true;
            } else {
                *sent_number -= 1;
            }
        }
        if delete {
            // remove data strictly older than the tick
            self.data
                .delete_old_data(tick, replication_group, component_registry);
            // remove all ack data older or equal to the tick
            self.acks
                .get_mut(&replication_group)
                .unwrap()
                .retain(|k, _| *k > tick);
        }
    }
}

type EntityHashMap<K, V> = hashbrown::HashMap<K, V, EntityHash>;

/// We have a shared store of the component values for diffable components.
/// We keep some of the values in memory so that we can compute the delta between the previously
/// send state and the current state.
/// We want this store to be shared across all ReplicationSenders (if there are multiple connections),
/// to avoid copying the component value for each connection
#[derive(Default)]
pub struct DeltaComponentStore {
    // TODO: maybe store the values on the components directly?
    data: EntityHashMap<
        ReplicationGroupId,
        // Using a vec seems faster than using nested HashMaps
        BTreeMap<Tick, Vec<(ComponentKind, Entity, NonNull<u8>)>>,
    >,
}

unsafe impl Send for DeltaComponentStore {}
unsafe impl Sync for DeltaComponentStore {}

impl DeltaComponentStore {
    pub(crate) fn store_component_value(
        &mut self,
        entity: Entity,
        tick: Tick,
        kind: ComponentKind,
        component: Ptr,
        replication_group: ReplicationGroupId,
        registry: &ComponentRegistry,
    ) {
        // SAFETY: the component Ptr corresponds to kind
        let cloned = unsafe { registry.erased_clone(component, kind).unwrap() };
        self.data
            .entry(replication_group)
            .or_default()
            .entry(tick)
            .or_default()
            .push((kind, entity, cloned));
    }

    pub(crate) fn get_component_value(
        &self,
        entity: Entity,
        tick: Tick,
        kind: ComponentKind,
        replication_group: ReplicationGroupId,
    ) -> Option<Ptr> {
        self.data
            .get(&replication_group)?
            .get(&tick)?
            .iter()
            .find_map(|(k, e, ptr)| {
                if *k == kind && *e == entity {
                    Some(unsafe { Ptr::new(*ptr) })
                } else {
                    None
                }
            })
    }

    pub(crate) fn delete_old_data(
        &mut self,
        tick: Tick,
        replication_group: ReplicationGroupId,
        registry: &ComponentRegistry,
    ) {
        if let Some(data) = self.data.get_mut(&replication_group) {
            // we can remove all the keys older than the acked key
            let recent_data = data.split_off(&tick).into_iter().collect();
            // call drop on all the data that we are removing
            data.values_mut().for_each(|tick_data| {
                tick_data.iter().for_each(|(kind, _, owned_ptr)| unsafe {
                    // SAFETY: the ptr corresponds to the kind
                    registry.erased_drop(*owned_ptr, *kind).unwrap();
                });
            });
            // only keep the data that is more recent (inclusive) than the acked tick
            *data = recent_data;
        }
    }
}
