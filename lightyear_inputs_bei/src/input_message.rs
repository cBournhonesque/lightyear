use crate::marker::InputMarker;
use crate::registry::{InputActionKind, InputRegistry};
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::ecs::entity::MapEntities;
use bevy::ecs::system::SystemParam;
use bevy::prelude::{EntityMapper, Res};
use bevy_enhanced_input::action_value::ActionValue;
use bevy_enhanced_input::input_context::InputContext;
use bevy_enhanced_input::prelude::{ActionState, Actions};
use core::cmp::max;
use core::fmt::{Debug, Formatter};
use lightyear_core::network::NetId;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{InputBuffer, InputData};
use lightyear_inputs::input_message::ActionStateSequence;
use serde::{Deserialize, Serialize};
use tracing::error;

// TODO: optimize by storing diffs between states
#[derive(Serialize, Deserialize)]
pub struct BEIStateSequence<C> {
    // TODO: use InputData for each action separately
    states: Vec<InputData<ActionsMessage>>,
    marker: core::marker::PhantomData<C>,
}

impl<C> PartialEq for BEIStateSequence<C> {
    fn eq(&self, other: &Self) -> bool {
        self.states == other.states
    }
}

impl<C> Debug for BEIStateSequence<C> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BEIStateSequence")
            .field("states", &self.states)
            .finish()
    }
}

impl<C> Clone for BEIStateSequence<C> {
    fn clone(&self) -> Self {
        Self {
            states: self.states.clone(),
            marker: core::marker::PhantomData,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct InputActionMessage {
    /// Unique network identifier for the InputAction
    pub net_id: NetId,
    pub state: ActionState,
    pub value: ActionValue,
    // Timing information. We include it in the snapshot in case of client rollbacks:
    // If a client rollbacks to a previous tick, they also need to rollback the timing information in order
    // to accurately replay the actions
    //
    // We don't need it in the message, but we include it because it lets us re-use the mesg to allocate a new data structure (new vecs) for the snapshot.
    pub elapsed_secs: Option<f32>,
    pub fired_secs: Option<f32>,
}

/// Instead of replicating the BEI Actions, we will replicate a serializable subset that can be used to
/// fully know on the remote client which actions should be triggered. This data will be used
/// to update the BEI `Actions` component
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
struct ActionsMessage {
    input_actions: Vec<InputActionMessage>,
}

impl ActionsMessage {
    // check if the message equals the snapshot while ignoring timing information
    fn equals_snapshot(&self, snapshot: &ActionsSnapshot) -> bool {
        self.input_actions
            .iter()
            .zip(snapshot.state.input_actions.iter())
            .all(|(action, snapshot)| {
                action.net_id == snapshot.net_id
                    && action.state == snapshot.state
                    && action.value == snapshot.value
            })
    }
}

/// Struct that stores a subset of [`Actions<C>`](Actions) that is needed to
/// reconstruct the actions state on the remote client.
///
/// This is different from the `ActionsMessage` because we:
/// - don't need to include timing information in the messages. Timing information will be computed on the remote directly
///   after setting the state/value using the message
///   (https://github.com/projectharmonia/bevy_enhanced_input/blob/55fc7bcd2b6fb2850cbb6a879e2620b786ae1de5/src/input_context/action_binding.rs#L321-L320)
/// - we need the timing information in the snapshot so that we can rollback the actions state on the client
///   to a previous state with accurate timing information
#[derive(Clone, PartialEq, Debug)]
pub struct ActionsSnapshot {
    state: ActionsMessage,
}

impl ActionsMessage {
    fn from_actions<C: InputContext>(actions: &Actions<C>, registry: &InputRegistry) -> Self {
        let input_actions = actions
            .iter()
            .map(|(type_id, action)| {
                
                let net_id = registry
                    .kind_map
                    .net_id(&InputActionKind::from(type_id))
                    .expect("Action must be registered in InputRegistry");
                InputActionMessage {
                    net_id: *net_id,
                    state: action.state,
                    value: action.value,
                    elapsed_secs: Some(action.elapsed_secs),
                    fired_secs: Some(action.fired_secs),
                }
            })
            .collect();
        Self { input_actions }
    }

    fn from_snapshot(snapshot: &ActionsSnapshot) -> Self {
        snapshot.state.clone()
    }

    #[allow(clippy::wrong_self_convention)]
    fn to_snapshot(self) -> ActionsSnapshot {
        ActionsSnapshot { state: self }
    }
}

impl<C: InputContext> ActionStateSequence for BEIStateSequence<C> {
    type Action = C;
    type Snapshot = ActionsSnapshot;

    type State = Actions<C>;

    type Marker = InputMarker<C>;

    type Context = Res<'static, InputRegistry>;

    fn is_empty(&self) -> bool {
        self.states.is_empty()
            || self
                .states
                .iter()
                // TODO: is this correct? we culd have all SameAsPrecedent but similar to something non-empty?
                .all(|s| matches!(s, InputData::Absent | InputData::SameAsPrecedent))
    }

    fn len(&self) -> usize {
        self.states.len()
    }

    fn update_buffer<'w, 's>(self, input_buffer: &mut InputBuffer<Self::Snapshot>, end_tick: Tick) {
        let start_tick = end_tick + 1 - self.len() as u16;
        // the first value is guaranteed to not be SameAsPrecedent
        for (delta, input) in self.states.into_iter().enumerate() {
            let tick = start_tick + Tick(delta as u16);
            match input {
                InputData::Absent => {
                    input_buffer.set_raw(tick, InputData::Absent);
                }
                InputData::SameAsPrecedent => {
                    input_buffer.set_raw(tick, InputData::SameAsPrecedent);
                }
                InputData::Input(input) => {
                    // do not set the value if it's equal to what's already in the buffer
                    if input_buffer.get(tick).is_some_and(|existing_value| {
                        // NOTE: we compare input to the snapshot so that we don't look at timing information!
                        input.equals_snapshot(existing_value)
                    }) {
                        continue;
                    }
                    input_buffer.set(tick, input.to_snapshot());
                }
            }
        }
    }

    fn build_from_input_buffer<'w, 's>(
        input_buffer: &InputBuffer<Self::Snapshot>,
        num_ticks: u16,
        end_tick: Tick,
    ) -> Option<Self> {
        let buffer_start_tick = input_buffer.start_tick?;
        // find the first tick for which we have an `ActionState` buffered
        let start_tick = max(end_tick - num_ticks + 1, buffer_start_tick);

        // find the initial state, (which we convert out of SameAsPrecedent)
        let start_state = input_buffer
            .get(start_tick)
            .map_or(InputData::Absent, |input| {
                InputData::Input(ActionsMessage::from_snapshot(input))
            });
        let mut states = vec![start_state];

        // append the other states until the end tick
        let buffer_start = (start_tick + 1 - buffer_start_tick) as usize;
        let buffer_end = (end_tick + 1 - buffer_start_tick) as usize;
        for idx in buffer_start..buffer_end {
            let state =
                input_buffer
                    .buffer
                    .get(idx)
                    .map_or(InputData::Absent, |input| match input {
                        InputData::Absent => InputData::Absent,
                        InputData::SameAsPrecedent => InputData::SameAsPrecedent,
                        InputData::Input(v) => InputData::Input(ActionsMessage::from_snapshot(v)),
                    });
            states.push(state);
        }
        Some(Self {
            states,
            marker: core::marker::PhantomData,
        })
    }

    fn to_snapshot<'w, 's>(
        state: &Self::State,
        registry: &<Self::Context as SystemParam>::Item<'w, 's>,
    ) -> Self::Snapshot {
        let actions_message = ActionsMessage::from_actions(state, registry);
        actions_message.to_snapshot()
    }

    fn from_snapshot<'w, 's>(
        state: &mut Self::State,
        snapshot: &Self::Snapshot,
        registry: &<Self::Context as SystemParam>::Item<'w, 's>,
    ) {
        snapshot.state.input_actions.iter().for_each(|action| {
            let Some(kind) = registry.kind_map.kind(action.net_id) else {
                error!(
                    "Action with net ID {:?} not found in InputRegistry",
                    action.net_id
                );
                return;
            };
            let Some(action_state) = state.get_mut(kind.0) else {
                error!("Action with type ID {:?} not found in Actions", kind);
                return;
            };
            action_state.state = action.state;
            action_state.value = action.value;
            if let Some(elapsed_secs) = action.elapsed_secs {
                action_state.elapsed_secs = elapsed_secs;
            };
            if let Some(fired_secs) = action.fired_secs {
                action_state.fired_secs = fired_secs;
            };
        });
    }
}

impl<A: MapEntities> MapEntities for BEIStateSequence<A> {
    fn map_entities<E: EntityMapper>(&mut self, entity_mapper: &mut E) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Reflect;
    use bevy_enhanced_input::prelude::{InputAction, InputContext};
    use core::any::TypeId;

    #[derive(InputContext)]
    struct Context1;

    #[derive(InputAction, Debug, Clone, PartialEq, Eq, Hash, Reflect)]
    #[input_action(output = bool)]
    struct Action1;

    #[test]
    fn test_create_message() {
        let mut registry = InputRegistry {
            kind_map: Default::default(),
        };
        registry.kind_map.add::<Action1>();
        let type_id = TypeId::of::<Action1>();
        let net_id = *registry
            .kind_map
            .net_id(&InputActionKind::from(type_id))
            .unwrap();
        let mut input_buffer = InputBuffer::default();
        let mut action_state = Actions::<Context1>::default();
        action_state.bind::<Action1>();

        input_buffer.set(
            Tick(2),
            ActionsSnapshot {
                state: ActionsMessage::from_actions(&action_state, &registry),
            },
        );
        let state = action_state.get_mut(type_id).unwrap();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        input_buffer.set(
            Tick(3),
            ActionsSnapshot {
                state: ActionsMessage::from_actions(&action_state, &registry),
            },
        );
        let state = action_state.get_mut(type_id).unwrap();
        state.state = ActionState::None;
        state.value = ActionValue::Bool(false);
        input_buffer.set(
            Tick(7),
            ActionsSnapshot {
                state: ActionsMessage::from_actions(&action_state, &registry),
            },
        );

        let sequence =
            BEIStateSequence::<Context1>::build_from_input_buffer(&input_buffer, 9, Tick(10))
                .unwrap();
        assert_eq!(
            sequence,
            BEIStateSequence::<Context1> {
                // tick 2
                states: vec![
                    InputData::Input(ActionsMessage {
                        input_actions: vec![InputActionMessage {
                            net_id,
                            state: ActionState::None,
                            value: ActionValue::Bool(false),
                            elapsed_secs: Some(0.0),
                            fired_secs: Some(0.0),
                        }],
                    }),
                    InputData::Input(ActionsMessage {
                        input_actions: vec![InputActionMessage {
                            net_id,
                            state: ActionState::Fired,
                            value: ActionValue::Bool(true),
                            elapsed_secs: Some(0.0),
                            fired_secs: Some(0.0),
                        }],
                    }),
                    InputData::SameAsPrecedent,
                    InputData::SameAsPrecedent,
                    InputData::SameAsPrecedent,
                    InputData::Input(ActionsMessage {
                        input_actions: vec![InputActionMessage {
                            net_id,
                            state: ActionState::None,
                            value: ActionValue::Bool(false),
                            elapsed_secs: Some(0.0),
                            fired_secs: Some(0.0),
                        }],
                    }),
                    InputData::Absent,
                    InputData::Absent,
                    InputData::Absent,
                ],
                marker: Default::default(),
            }
        );
    }
}
