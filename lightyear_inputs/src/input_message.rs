// lightyear_inputs/src/input_message.rs
#![allow(clippy::module_inception)]
use crate::input_buffer::InputBuffer;
#[cfg(not(feature = "std"))]
use alloc::{format, string::String, vec, vec::Vec};
use bevy::ecs::component::Mutable;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{Component, Entity, EntityMapper, Reflect};
use core::fmt::{Debug, Formatter, Write};
use lightyear_core::prelude::Tick;
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
    pub(crate) target: InputTarget,
    /// Input data from ticks `end_tick - N + 1` to `end_tick` (inclusive).
    /// The format depends on the specific input system (e.g., full states or diffs).
    /// For simplicity in the base crate, we'll store it as `Vec<InputData<A>>`.
    /// Specific implementations (native, leafwing) will handle conversion.
    pub(crate) states: S,
}

pub trait ActionStateSequence:
    Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static
{
    type Action: Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + Debug + 'static;
    type State: Component<Mutability = Mutable> + Default + Debug + Clone + PartialEq;

    /// Marker component to identify the ActionState that the player is actively updating
    /// (as opposed to the ActionState of other players, for instance)
    type Marker: Component;

    fn is_empty(&self) -> bool;
    fn len(&self) -> usize;

    fn update_buffer(&self, input_buffer: &mut InputBuffer<Self::State>, end_tick: Tick);

    fn build_from_input_buffer(
        input_buffer: &InputBuffer<Self::State>,
        num_ticks: u16,
        end_tick: Tick,
    ) -> Option<Self>
    where
        Self: Sized;
}

/// Message used to send client inputs to the server.
/// Stores the last N inputs starting from `end_tick - N + 1`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
pub struct InputMessage<S> {
    // TODO: add interpolation delay
    pub(crate) end_tick: Tick,
    // Map from target entity to the input data for that entity
    pub(crate) inputs: Vec<PerTargetData<S>>,
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
            return write!(f, "EmptyInputMessage<{:?}>", ty);
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
            "InputMessage<{:?}> (End Tick: {:?}):\n{}",
            ty, self.end_tick, buffer_str
        )
    }
}

impl<S: ActionStateSequence> InputMessage<S> {
    pub fn new(end_tick: Tick) -> Self {
        Self {
            end_tick,
            inputs: vec![],
        }
    }

    /// Checks if the message contains any actual input data.
    pub fn is_empty(&self) -> bool {
        self.inputs.iter().all(|data| data.states.is_empty())
    }
}

// TODO: Define traits `InputMessageBuilder<A>` and `InputBufferUpdater<A>` here
//       and implement them in the respective native/leafwing crates.

#[cfg(test)]
mod tests {
    // Tests for InputMessage construction and basic properties can remain here,
    // but tests involving `add_inputs` or `update_from_message` need to be
    // moved or adapted in the native/leafwing crates.
    use super::*;
    use crate::input_buffer::InputData;
    use lightyear_core::prelude::Tick;

    #[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
    struct MyTestAction(u32);
    impl MapEntities for MyTestAction {
        fn map_entities<M: EntityMapper>(&mut self, _entity_mapper: &mut M) {}
    }

    #[test]
    fn test_input_message_empty() {
        let msg_empty: InputMessage<MyTestAction> = InputMessage::new(Tick(10));
        assert!(msg_empty.is_empty());

        let msg_absent = InputMessage {
            end_tick: Tick(10),
            inputs: vec![PerTargetData {
                target: InputTarget::Entity(Entity::PLACEHOLDER),
                states: vec![InputData::Absent, InputData::SameAsPrecedent],
            }],
        };
        assert!(msg_absent.is_empty());

        let msg_present = InputMessage {
            end_tick: Tick(10),
            inputs: vec![PerTargetData {
                target: InputTarget::Entity(Entity::PLACEHOLDER),
                states: vec![InputData::Absent, InputData::Input(MyTestAction(1))],
            }],
        };
        assert!(!msg_present.is_empty());
    }
}
