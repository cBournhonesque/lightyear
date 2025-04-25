//! Logic related to delta compression (sending only the changes between two states, instead of the new state)

use crate::prelude::{ComponentRegistry, Message, Tick};
use crate::protocol::component::ComponentKind;
use crate::shared::replication::components::ReplicationGroupId;
use bevy::platform::collections::HashMap;
use bevy::ecs::entity::EntityHash;
use bevy::prelude::{Component, Entity};
use bevy::ptr::Ptr;

use alloc::collections::BTreeMap;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use core::ptr::NonNull;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum DeltaType {
    /// This delta is computed from a previous value
    Normal {
        /// The tick of the previous state
        previous_tick: Tick,
    },
    /// This delta is computed from the Base value
    FromBase,
}

/// A message that contains a delta between two states (for serializing delta compression)
// Need repr(C) to be able to cast the pointer to a u8 pointer
#[repr(C)]
#[derive(Component, Deserialize, Serialize)]
pub struct DeltaMessage<M> {
    pub(crate) delta_type: DeltaType,
    pub(crate) delta: M,
}

/// A type is Diffable when you can:
/// - Compute the delta between two states
/// - Apply the delta to an old state to get the new state
///
/// Some examples could be:
/// - your component contains a hashmap, and your delta is `Add(key, value)` and `Remove(key)`
/// - your component is a struct with multiple fields, and your delta only contains data for the fields that changed.
///   (to avoid sending the full struct every time over the network)
pub trait Diffable: Clone {
    // /// Set to true if the Deltas are idempotent (applying the same delta multiple times has no effect)
    // const IDEMPOTENT: bool;
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

/// Store a history of past delta-component values so we can apply diffs properly
#[derive(Component, Debug)]
pub struct DeltaComponentHistory<C> {
    // We cannot use a ReadyBuffer because we need to be able to fetch values at arbitrary ticks
    // not just the most recent ticks
    pub buffer: BTreeMap<Tick, C>,
}

// Implementing Default manually to not require C: Default
impl<C> Default for DeltaComponentHistory<C> {
    fn default() -> Self {
        Self {
            buffer: BTreeMap::new(),
        }
    }
}

#[derive(Default, Debug)]
pub struct DeltaManager {
    pub(crate) data: DeltaComponentStore,
    /// Keeps track of how many clients have acked a specific tick for a specific replication group
    /// Used to track when we can drop old data.
    // TODO: maybe we don't need this and we can just drop data after 100 tick?
    // TODO: this should be per component! especially if do the send updates since `send_tick` which needs
    //  to be done since (entity, component)
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
        // check if we can remove the stored data because all clients have acked this tick
        let mut delete = false;
        if let Some(group_data) = self.acks.get_mut(&replication_group) {
            if let Some(sent_number) = group_data.get_mut(&tick) {
                if *sent_number == 1 {
                    // TODO: maybe optimize this by keeping track in each message of which delta compression components were included?
                    // all the clients have acked this message, we can remove the data for all ticks older than this one
                    delete = true;
                } else {
                    *sent_number -= 1;
                }
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

    /// To avoid tick-wrapping issues, we run a system regularly (every u16::MAX / 3 ticks)
    /// to clean up old tick data.
    ///
    /// We remove every tick that is too old (which means we cannot do delta compression and
    /// we will be sending a full component value)
    pub(crate) fn tick_cleanup(&mut self, current_tick: Tick) {
        let delta = (u16::MAX / 3) as i16;
        self.acks.values_mut().for_each(|group_data| {
            group_data.retain(|k, _| *k - current_tick > delta);
        });
        self.data.data.values_mut().for_each(|group_data| {
            group_data.retain(|k, _| *k - current_tick > delta);
        });
    }
}

type EntityHashMap<K, V> = bevy::platform::collections::HashMap<K, V, EntityHash>;

/// We have a shared store of the component values for diffable components.
/// We keep some of the values in memory so that we can compute the delta between the previously
/// send state and the current state.
/// We want this store to be shared across all ReplicationSenders (if there are multiple connections),
/// to avoid copying the component value for each connection
#[derive(Default, Debug)]
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

#[cfg(test)]
mod tests {
    #[cfg(not(feature = "std"))]
    use alloc::vec;
    use super::*;
    use crate::tests::protocol::ComponentDeltaCompression;
    use bevy::prelude::World;

    #[test]
    fn test_add_get_data() {
        let mut world = World::default();
        let mut registry = ComponentRegistry::default();
        registry.register_component::<ComponentDeltaCompression>(&mut world);
        registry.set_delta_compression::<ComponentDeltaCompression>(&mut world);
        let mut store = DeltaComponentStore::default();
        let entity = Entity::from_raw(0);
        let tick = Tick(0);
        let replication_group = ReplicationGroupId(0);
        let component = ComponentDeltaCompression(vec![1, 2]);
        let ptr = Ptr::from(&component);
        let kind = ComponentKind::of::<ComponentDeltaCompression>();

        store.store_component_value(entity, tick, kind, ptr, replication_group, &registry);

        let retrieved = store
            .get_component_value(entity, tick, kind, replication_group)
            .unwrap();
        let retrieved_component = unsafe { retrieved.deref::<ComponentDeltaCompression>() };
        assert_eq!(retrieved_component, &component);
    }

    #[test]
    fn test_delete_old_data() {
        let mut world = World::default();
        let mut registry = ComponentRegistry::default();
        registry.register_component::<ComponentDeltaCompression>(&mut world);
        registry.set_delta_compression::<ComponentDeltaCompression>(&mut world);
        let mut store = DeltaComponentStore::default();
        let entity = Entity::from_raw(0);
        let tick_1 = Tick(1);
        let tick_2 = Tick(2);
        let tick_3 = Tick(3);
        let replication_group = ReplicationGroupId(0);
        let component = ComponentDeltaCompression(vec![1, 2]);
        let ptr = Ptr::from(&component);
        let kind = ComponentKind::of::<ComponentDeltaCompression>();

        store.store_component_value(entity, tick_1, kind, ptr, replication_group, &registry);
        store.store_component_value(entity, tick_2, kind, ptr, replication_group, &registry);
        store.store_component_value(entity, tick_3, kind, ptr, replication_group, &registry);

        store.delete_old_data(tick_2, replication_group, &registry);

        assert!(store
            .get_component_value(entity, tick_1, kind, replication_group)
            .is_none());
        let retrieved = store
            .get_component_value(entity, tick_2, kind, replication_group)
            .unwrap();
        let retrieved_component = unsafe { retrieved.deref::<ComponentDeltaCompression>() };
        assert_eq!(retrieved_component, &component);
        let retrieved = store
            .get_component_value(entity, tick_3, kind, replication_group)
            .unwrap();
        let retrieved_component = unsafe { retrieved.deref::<ComponentDeltaCompression>() };
        assert_eq!(retrieved_component, &component);
    }
}
