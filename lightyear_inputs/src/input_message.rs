// lightyear_inputs/src/input_message.rs
#![allow(type_alias_bounds)]
#![allow(clippy::module_inception)]
use crate::input_buffer::{InputBuffer, InputData};
use alloc::{format, string::String, vec, vec::Vec};
use bevy_app::App;
use bevy_ecs::bundle::Bundle;
use bevy_ecs::query::QueryData;
use bevy_ecs::{
    component::Component,
    entity::{Entity, EntityMapper, MapEntities},
};
use bevy_reflect::Reflect;
use core::fmt::{Debug, Formatter, Write};
use core::time::Duration;
use bevy_utils::prelude::DebugName;
use lightyear_core::prelude::Tick;
#[cfg(feature = "interpolation")]
use lightyear_interpolation::plugin::InterpolationDelay;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use tracing::{debug, error, info, trace};

/// Enum indicating the target entity for the input.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Reflect)]
pub enum InputTarget {
    /// The input is for a predicted or confirmed entity.
    /// When sending from client to server, entity mapping is applied.
    /// (Also when rebroadcast from server to client)
    Entity(Entity),
    /// The input is for a prespawned entity.
    /// We wan the client to be able to send inputs for a prespawned entity before it gets matched with a server entity.
    /// To achieve this, the client sends the PreSpawned hash and the server will map it to the correct server entity.
    /// When rebroadcasting from server to other client, we rebroadcast it as a normal Entity?
    PreSpawned(u64),
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
    fn decay_tick(&mut self, tick_duration: Duration);
}

/// A QueryData that contains the queryable state that contains the current state of the Action at the given tick
///
/// This is used to simply be `ActionState<A>`, but BEI switched to representing the action state with multiple components,
/// so this traits abstracts over that difference.
pub trait ActionStateQueryData {
    // The mutable QueryData that allows modifying the ActionState.
    // If the ActionState is a single component, then this is simply `&'static mut Self`.
    type Mut: QueryData;

    // The inner value corresponding to Self::Mut::Item<'w, 's> (i.e. for Mut<'w, 's, ActionState<A>, this is &'mut ActionState<A>)
    type MutItemInner<'w>;

    // Component that should always be present to represent the ActionState.
    // We use this for registering required components in the App.
    type Main: Component + Send + Sync + Default + 'static;

    // Bundle that contains all the components needed to represent the ActionState.
    type Bundle: Bundle + Send + Sync + 'static;

    // Convert from the mutable query item (i.e. Mut<'w, ActionState<A>>) to the read-only query item (i.e. &ActionState<A>)
    fn as_read_only<'a, 'w: 'a, 's>(
        state: &'a <Self::Mut as QueryData>::Item<'w, 's>,
    ) -> <<Self::Mut as QueryData>::ReadOnly as QueryData>::Item<'a, 's>;

    // Convert from the mutable query item (i.e. Mut<'w, ActionState<A>>) to the inner mutable item (i.e. &mut ActionState<A>)
    fn into_inner<'w, 's>(
        mut_item: <Self::Mut as QueryData>::Item<'w, 's>,
    ) -> Self::MutItemInner<'w>;

    // Convert from the Bundle (ActionState<A>) to the inner mutable item (i.e. &mut ActionState<A>)
    fn as_mut<'w>(bundle: &'w mut Self::Bundle) -> Self::MutItemInner<'w>;
    fn base_value() -> Self::Bundle;
}

// equivalent to &ActionState<S::Action>
pub(crate) type StateRef<S: ActionStateSequence> =
    <<S::State as ActionStateQueryData>::Mut as QueryData>::ReadOnly;

// equivalent to &'w ActionState<S::Action>
pub(crate) type StateRefItem<'w, 's, S: ActionStateSequence> =
    <StateRef<S> as QueryData>::Item<'w, 's>;

// equivalent to &mut ActionState<S::Action>
pub(crate) type StateMut<S: ActionStateSequence> = <S::State as ActionStateQueryData>::Mut;

// equivalent to Mut<'w, ActionState<S::Action>>
pub(crate) type StateMutItem<'w, 's, S: ActionStateSequence> =
    <StateMut<S> as QueryData>::Item<'w, 's>;

pub(crate) type StateMutItemInner<'w, S: ActionStateSequence> =
    <S::State as ActionStateQueryData>::MutItemInner<'w>;

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

    fn is_empty(&self) -> bool;
    fn len(&self) -> usize;

    /// Register the required components for this ActionStateSequence in the App.
    fn register_required_components(app: &mut App) {
        // TODO: cannot create cyclic required dependencies in bevy 0.17
        // app.register_required_components::<<Self::State as ActionStateQueryData>::Main, InputBuffer<Self::Snapshot>>();
        app.register_required_components::<InputBuffer<Self::Snapshot>, <Self::State as ActionStateQueryData>::Main>();
        app.register_required_components::<Self::Marker, InputBuffer<Self::Snapshot>>();
    }

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
        tick_duration: Duration,
    ) -> Option<Tick> {
        let previous_end_tick = input_buffer.end_tick();

        let mut previous_predicted_input = input_buffer.get_last().cloned();
        let mut earliest_mismatch: Option<Tick> = None;
        let start_tick = end_tick + 1 - self.len() as u16;

        // the first value is guaranteed to not be SameAsPrecedent
        for (delta, input) in self.get_snapshots_from_message().enumerate() {
            let tick = start_tick + Tick(delta as u16);

            // TODO: instead of doing this every time, should we just keep updating/mocking the inputs for the remote clients?
            //  then the buffer would be filled with predicted inputs up to the current tick
            // for ticks after the last tick in the buffer, we start decaying our previous_predicted_input
            if previous_end_tick.is_some_and(|t| tick > t) {
                previous_predicted_input = previous_predicted_input.map(|prev| {
                    let mut prev = prev;
                    prev.decay_tick(tick_duration);
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
                        // no mismatch but this is a tick after our previous_end_tick so we want to add it to the buffer.
                        input_buffer.set_raw(tick, input);
                        continue;
                    }
                    // first mismatch tick!
                    // set the new value for the mismatch tick
                    debug!(
                        "Mismatch detected at tick {tick:?} for new_input {input:?}. Previous predicted input: {previous_predicted_input:?}"
                    );
                    input_buffer.set_raw(tick, input);
                    earliest_mismatch = Some(tick);
                }
            }
        }
        trace!("input buffer after update: {input_buffer:?}");
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
    fn to_snapshot<'w, 's>(state: StateRefItem<'w, 's, Self>) -> Self::Snapshot;

    /// Modify the given state to reflect the given snapshot.
    fn from_snapshot<'w>(state: StateMutItemInner<'w, Self>, snapshot: &Self::Snapshot);
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
            match &mut data.target {
                InputTarget::Entity(e) => {
                    *e = entity_mapper.get_mapped(*e);
                }
                _ => {}
            }
            data.states.map_entities(entity_mapper);
        });
    }
}

impl<S: ActionStateSequence + core::fmt::Display> core::fmt::Display for InputMessage<S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        let ty = DebugName::type_name::<S::Action>();

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
