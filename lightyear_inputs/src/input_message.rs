// lightyear_inputs/src/input_message.rs
#![allow(clippy::module_inception)]
use crate::input_buffer::InputBuffer;
use alloc::{format, string::String, vec, vec::Vec};
use bevy_ecs::{
    component::{Component, Mutable},
    entity::{Entity, EntityMapper, MapEntities},
    system::SystemParam,
};
use bevy_reflect::Reflect;
use core::fmt::{Debug, Formatter, Write};
use lightyear_core::prelude::Tick;
#[cfg(feature = "interpolation")]
use lightyear_interpolation::plugin::InterpolationDelay;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

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

pub trait ActionStateSequence:
    Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static
{
    /// The type of the Action
    type Action: Send + Sync + 'static;

    /// Snapshot of the State that will be stored in the InputBuffer.
    /// This should be enough to be able to reconstruct the State at a given tick.
    type Snapshot: Send + Sync + Debug + PartialEq + Clone + 'static;

    /// The component that is used by the user to get the list of active actions.
    type State: Component<Mutability = Mutable> + Default;

    /// Marker component to identify the ActionState that the player is actively updating
    /// (as opposed to the ActionState of other players, for instance)
    type Marker: Component;

    /// Extra context that needs to be fetched and is needed to build the state sequence from the input buffer
    type Context: SystemParam;

    fn is_empty(&self) -> bool;
    fn len(&self) -> usize;

    /// Update the given input buffer with the data from this state sequence
    fn update_buffer(self, input_buffer: &mut InputBuffer<Self::Snapshot>, end_tick: Tick);

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
        state: &Self::State,
        context: &<Self::Context as SystemParam>::Item<'w, 's>,
    ) -> Self::Snapshot;

    /// Modify the given state to reflect the given snapshot.
    fn from_snapshot<'w, 's>(
        state: &mut Self::State,
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
