// lightyear_inputs/src/input_message.rs
#![allow(clippy::module_inception)]
use crate::input_buffer::{InputBuffer, InputData};
use alloc::{format, string::String, vec, vec::Vec};
use bevy_ecs::{
    component::{Component, Mutable},
    entity::{Entity, EntityMapper, MapEntities},
    system::SystemParam,
};
use bevy_reflect::Reflect;
use core::fmt::{Debug, Formatter, Write};
use std::ops::Deref;
use bevy_ecs::bundle::Bundle;
use bevy_ecs::query::{QueryData, ReadOnlyQueryData};
use lightyear_core::prelude::Tick;
#[cfg(feature = "interpolation")]
use lightyear_interpolation::plugin::InterpolationDelay;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Enum indicating the target entity for the input.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Reflect)]
pub enum InputTarget {
    /// The input is for a predicted or confirmed entity.
    /// On the client, the server's local entity is mapped to the client's confirmed entity.
    Entity(Entity),
    /// The input is for a pre-predicted entity.
    /// On the server, the server's local entity is mapped to the client's pre-predicted entity.
    PrePredictedEntity(Entity),
}

/// Contains the input data for a specific target entity over a range of ticks.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
pub struct PerTargetData<S> {
    pub target: InputTarget,
    /// Input data from ticks `end_tick - N + 1` to `end_tick` (inclusive).
    /// The format depends on the specific input system (e.g., full states or diffs).
    /// For simplicity in the base crate, we'll store it as `Vec<InputData<A>>`.
    /// Specific implementations (native, leafwing) will handle conversion.
    pub states: S,
}

pub trait InputSnapshot: Send + Sync + Debug + Clone + PartialEq + 'static {
    /// The type of the Action that this snapshot represents.
    type Action: Send + Sync + 'static;

    /// Predict the next snapshot after 1 tick.
    ///
    /// By default Snapshots do not decay, i.e. we predict that they stay the same and the user
    /// keeps pressing the same button.
    fn decay_tick(&mut self);
}


/// A QueryData that contains the queryable state that contains the current state of the Action at the given tick
/// 
/// This is usually a single component (ActionState) or multiple components in the case of BEI (ActionState, ActionValue, etc.)
pub trait ActionStateQueryData: QueryData
where
    for<'a> <Self as QueryData>::Item<'a>: Deref<Target=<Self::ReadOnly as QueryData>::Item<'a>>,
{
    type Bundle: Bundle + Send + Sync + Default + 'static;
    fn base_value() -> Self::Bundle;
}

// pub trait ActionStateQueryDataMut {
//     type Mut;
//     fn as_mut(&mut self) -> &mut Self::Mut;
// }

/// An ActionStateSequence represents a sequence of states that can be serialized and sent over the network.
///
/// The sequence can be decoded back into a `Iterator<Item = InputData<Self::Snapshot>>`
pub trait ActionStateSequence:
    Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static
{
    /// The type of the Action
    type Action: Send + Sync + 'static;

    /// Snapshot of the State that will be stored in the InputBuffer.
    /// This should be enough to be able to reconstruct the State at a given tick.
    type Snapshot: InputSnapshot<Action = Self::Action>;

    /// The component that is used by the user to get the list of active actions.
    type State: ActionStateQueryData;


    /// Marker component to identify the ActionState that the player is actively updating
    /// (as opposed to the ActionState of other players, for instance)
    type Marker: Component;

    /// Extra context that needs to be fetched and is needed to build the state sequence from the input buffer
    type Context: SystemParam;

    fn is_empty(&self) -> bool;
    fn len(&self) -> usize;

    /// Returns the sequence of snapshots from the ActionStateSequence.
    ///
    /// (we use this function instead of making ActionStateSequence implement `IntoIterator` because that would
    /// leak private types that are used in the IntoIter type)
    fn get_snapshots_from_message(self) -> impl Iterator<Item = InputData<Self::Snapshot>>;

    /// Update the given input buffer with the data from this state sequence.
    ///
    /// Returns the earliest tick where there is a mismatch between the existing buffer and the new data,
    /// or None if there was no mismatch
    fn update_buffer(
        self,
        input_buffer: &mut InputBuffer<Self::Snapshot>,
        end_tick: Tick,
    ) -> Option<Tick> {
        let previous_end_tick = input_buffer.end_tick();

        let mut previous_predicted_input = input_buffer.get_last().cloned();
        let mut earliest_mismatch: Option<Tick> = None;
        let start_tick = end_tick + 1 - self.len() as u16;

        // the first value is guaranteed to not be SameAsPrecedent
        for (delta, input) in self.get_snapshots_from_message().enumerate() {
            let tick = start_tick + Tick(delta as u16);

            // for ticks after the last tick in the buffer, we start decaying our previous_predicted_input
            if previous_end_tick.is_some_and(|t| tick > t) {
                previous_predicted_input = previous_predicted_input.map(|prev| {
                    let mut prev = prev;
                    prev.decay_tick();
                    prev
                });
            }

            // after the mismatch, we just fill with the data from the message
            if earliest_mismatch.is_some() {
                input_buffer.set_raw(tick, input);
            } else {
                // only try to detect mismatches after the previous_end_tick
                if previous_end_tick.is_none_or(|t| tick > t) {
                    if previous_end_tick.is_some()
                        && match (&previous_predicted_input, &input) {
                            // it is not possible to get a mismatch from SameAsPrecedent without first getting a mismatch from Input or Absent
                            (_, InputData::SameAsPrecedent) => true,
                            (Some(prev), InputData::Input(latest)) => latest == prev,
                            (None, InputData::Absent) => true,
                            _ => false,
                        }
                    {
                        continue;
                    }
                    // mismatch! fill the ticks between previous_end_tick and this tick
                    if let Some(prev_end) = previous_end_tick {
                        for delta in 1..(tick - prev_end) {
                            input_buffer.set_raw(prev_end + delta, InputData::SameAsPrecedent);
                        }
                    }
                    // set the new value for the mismatch tick
                    debug!(
                        "Mismatch detected at tick {tick:?} for new_input {input:?}. Previous predicted input: {previous_predicted_input:?}"
                    );
                    input_buffer.set_raw(tick, input);
                    earliest_mismatch = Some(tick);
                }
            }
        }

        // if there was 0 mismatch, fill the gap between previous_end_tick and end_tick
        if earliest_mismatch.is_none()
            && let Some(prev_end) = previous_end_tick
        {
            for delta in 1..(end_tick - prev_end + 1) {
                input_buffer.set_raw(prev_end + delta, InputData::SameAsPrecedent);
            }
        }
        debug!("input buffer after update: {input_buffer:?}");
        earliest_mismatch
    }

    /// Build the state sequence (which will be sent over the network) from the input buffer
    fn build_from_input_buffer(
        input_buffer: &InputBuffer<Self::Snapshot>,
        num_ticks: u16,
        end_tick: Tick,
    ) -> Option<Self>
    where
        Self: Sized;

    /// Create a snapshot from the given state.
    fn to_snapshot<'w, 's>(
        // state: AsRef<Self::State::ReadOnly::Item<'w>>,
        state: <Self::State as QueryData>::Item<'w>,
        context: &<Self::Context as SystemParam>::Item<'w, 's>,
    ) -> Self::Snapshot;

    /// Modify the given state to reflect the given snapshot.
    fn from_snapshot<'w, 's>(
        state: <Self::State as QueryData>::Item<'w>,
        snapshot: &Self::Snapshot,
        context: &<Self::Context as SystemParam>::Item<'w, 's>,
    );
}

/// Message used to send client inputs to the server.
/// Stores the last N inputs starting from `end_tick - N + 1`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
pub struct InputMessage<S> {
    #[cfg(feature = "interpolation")]
    /// Interpolation delay of the client at the time the message is sent
    ///
    /// We don't need any extra redundancy for the InterpolationDelay so we'll just send the value at `end_tick`.
    pub interpolation_delay: Option<InterpolationDelay>,
    pub end_tick: Tick,
    // Map from target entity to the input data for that entity
    pub inputs: Vec<PerTargetData<S>>,
}

impl<S: ActionStateSequence + MapEntities> MapEntities for InputMessage<S> {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.inputs.iter_mut().for_each(|data| {
            // Only map PrePredictedEntity targets during message deserialization
            if let InputTarget::PrePredictedEntity(e) = &mut data.target {
                *e = entity_mapper.get_mapped(*e);
            }
            data.states.map_entities(entity_mapper);
        });
    }
}

impl<S: ActionStateSequence + core::fmt::Display> core::fmt::Display for InputMessage<S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        let ty = core::any::type_name::<S::Action>();

        if self.inputs.is_empty() {
            return write!(f, "EmptyInputMessage<{ty:?}>");
        }
        let buffer_str = self
            .inputs
            .iter()
            .map(|data| {
                let mut str = format!("Target: {:?}\n", data.target);
                let _ = writeln!(&mut str, "States: {}", data.states);
                str
            })
            .collect::<Vec<String>>()
            .join("\n");
        write!(
            f,
            "InputMessage<{ty:?}> (End Tick: {:?}):\n{buffer_str}",
            self.end_tick
        )
    }
}

impl<S: ActionStateSequence> InputMessage<S> {
    pub fn new(end_tick: Tick) -> Self {
        Self {
            #[cfg(feature = "interpolation")]
            interpolation_delay: None,
            end_tick,
            inputs: vec![],
        }
    }

    /// Checks if the message contains any actual input data.
    pub fn is_empty(&self) -> bool {
        self.inputs.iter().all(|data| data.states.is_empty())
    }
}
