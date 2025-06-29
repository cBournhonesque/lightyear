//! Logic related to delta compression (sending only the changes between two states, instead of the new state)

use crate::registry::ComponentKind;
use crate::registry::registry::ComponentRegistry;
use alloc::collections::BTreeMap;
use bevy_ecs::{component::Component, entity::Entity};
use bevy_ptr::Ptr;
use core::ptr::NonNull;
use dashmap::DashMap;
use lightyear_core::prelude::Tick;
use lightyear_messages::Message;
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

#[derive(Debug)]
struct PerTickData {
    /// The data for each tick, stored as a NonNull<u8> pointer to the component value
    /// This is used to avoid copying the component value for each connection
    ///
    /// The data will be used to compute deltas between the last sent state and the current state.
    ptr: NonNull<u8>,
    /// The number of remote peers that we have yet to receive an ack from.
    /// Incremented by 1 when we send the component.
    /// Decremented by 1 when receive the ack.
    /// When it reaches 0, we can delete the data for this tick and all older ticks.
    num_acks: usize,
}

#[derive(Debug, Default)]
struct PerComponentData {
    /// The data for each tick, stored as a NonNull<u8> pointer to the component value
    /// This is used to avoid copying the component value for each connection
    ///
    /// We also store the number of remote peers that we have sent the component value
    /// for this tick to.
    ///
    /// The data will be used to compute deltas between the last sent state and the current state.
    data: BTreeMap<Tick, PerTickData>,
}

unsafe impl Send for PerComponentData {}
unsafe impl Sync for PerComponentData {}

// TODO: handle TickSyncEvent
/// Component that will manage keeping the old state of diffable components
/// so that the sender can compute deltas between the last sent state and the current state.
///
/// The state is shared between all ReplicationSenders that use the same DeltaManager.
#[derive(Default, Component, Debug)]
pub struct DeltaManager {
    // TODO: how to do we cleanup old keys?
    state: DashMap<(ComponentKind, Entity), PerComponentData>,
}

impl DeltaManager {
    /// Notify the DeltaManager that we are sending an update for a specific entity and component.
    ///
    /// - Store the component value for this tick, so that we can compute deltas from it later
    /// - Remember that we sent this tick for this entity and component, so that we can track how many clients have acked it.
    pub(crate) fn store(
        &self,
        entity: Entity,
        tick: Tick,
        kind: ComponentKind,
        component: Ptr,
        registry: &ComponentRegistry,
    ) {
        self.state
            .entry((kind, entity))
            .or_default()
            .data
            .entry(tick)
            .or_insert_with(|| {
                PerTickData {
                    // SAFETY: the component Ptr corresponds to kind
                    ptr: unsafe { registry.erased_clone(component, kind).unwrap() },
                    num_acks: 0,
                }
            })
            .num_acks += 1;
    }

    /// Get the stored component value so that we can compute deltas from it.
    pub(crate) fn get(&self, entity: Entity, tick: Tick, kind: ComponentKind) -> Option<Ptr> {
        let tick_data = self.state.get(&(kind, entity))?;
        let ptr = tick_data.data.get(&tick)?;
        Some(unsafe { Ptr::new(ptr.ptr) })
    }

    /// We receive an ack from a client for a specific tick.
    /// Update the ack information;
    pub(crate) fn receive_ack(
        &mut self,
        entity: Entity,
        tick: Tick,
        kind: ComponentKind,
        registry: &ComponentRegistry,
    ) {
        if let Some(mut group_data) = self.state.get_mut(&(kind, entity)) {
            if let Some(per_tick_data) = group_data.data.get_mut(&tick) {
                if per_tick_data.num_acks == 1 {
                    // TODO: maybe optimize this by keeping track in each message of which delta compression components were included?

                    // if all clients have acked this tick, we can remove the data
                    // for all ticks older or equal to this one

                    // we can remove all the keys older or equal than the acked key
                    let recent_data = group_data.data.split_off(&(tick + 1));
                    // call drop on all the data that we are removing
                    group_data.data.values_mut().for_each(|tick_data| {
                        // SAFETY: the ptr corresponds to the correct kind
                        unsafe { registry.erased_drop(tick_data.ptr, kind) }
                            .expect("unable to drop component value");
                    });
                    group_data.data = recent_data;
                } else {
                    per_tick_data.num_acks -= 1;
                }
            }
        }
    }

    /// To avoid tick-wrapping issues, we run a system regularly (every u16::MAX / 3 ticks)
    /// to clean up old tick data.
    ///
    /// We remove every tick that is too old (which means we cannot do delta compression and
    /// we will be sending a full component value)
    pub(crate) fn tick_cleanup(&mut self, current_tick: Tick) {
        let delta = (u16::MAX / 3) as i16;
        self.state.alter_all(|_, mut group_data| {
            group_data.data.retain(|k, _| *k - current_tick > delta);
            group_data
        });
    }
}
