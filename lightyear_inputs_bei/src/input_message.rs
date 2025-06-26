use crate::marker::InputMarker;
use crate::registry::{InputActionKind, InputRegistry};
use alloc::{vec, vec::Vec};
use bevy_ecs::{
    entity::{EntityMapper, MapEntities},
    system::{Res, SystemParam},
};
use bevy_enhanced_input::action_value::ActionValue;
use bevy_enhanced_input::input_context::InputContext;
use bevy_enhanced_input::prelude::{ActionEvents, ActionState, Actions};
use core::cmp::max;
use core::fmt::{Debug, Formatter};
use lightyear_core::network::NetId;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{InputBuffer, InputData};
use lightyear_inputs::input_message::ActionStateSequence;
use serde::{Deserialize, Serialize};
use tracing::{error, trace};

pub struct BEIStateSequence<C> {
    // TODO: use InputData for each action separately to optimize the diffs
    states: Vec<InputData<ActionsMessage>>,
    marker: core::marker::PhantomData<C>,
}

impl<C> Serialize for BEIStateSequence<C> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.states.serialize(serializer)
    }
}

impl<'de, C> Deserialize<'de> for BEIStateSequence<C> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let states = Vec::<InputData<ActionsMessage>>::deserialize(deserializer)?;
        Ok(Self {
            states,
            marker: core::marker::PhantomData,
        })
    }
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

impl<C> MapEntities for BEIStateSequence<C> {
    fn map_entities<E: EntityMapper>(&mut self, entity_mapper: &mut E) {}
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct InputActionMessage {
    /// Unique network identifier for the InputAction
    pub net_id: NetId,
    pub state: ActionState,
    pub value: ActionValue,
    pub events: ActionEvents,
    pub elapsed_secs: f32,
    pub fired_secs: f32,
    // // Timing information. We include it in the snapshot in case of client rollbacks:
    // // If a client rollbacks to a previous tick, they also need to rollback the timing information in order
    // // to accurately replay the actions
    // //
    // // We don't need it in the message, but we include it because it lets us re-use the mesg to allocate a new data structure (new vecs) for the snapshot.
    // pub elapsed_secs: Option<f32>,
    // pub fired_secs: Option<f32>,
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
    fn equals_snapshot<C>(&self, snapshot: &ActionsSnapshot<C>) -> bool {
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
/// We need the timing information in the snapshot so that we can rollback the actions state on the client
/// to a previous state with accurate timing information, or when we fetch a previous actions state if
/// input_delay is enabled
pub struct ActionsSnapshot<C> {
    state: ActionsMessage,
    _marker: core::marker::PhantomData<C>,
}

impl<C> Clone for ActionsSnapshot<C> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            _marker: core::marker::PhantomData,
        }
    }
}

impl<C> PartialEq for ActionsSnapshot<C> {
    fn eq(&self, other: &Self) -> bool {
        self.state.eq(&other.state)
    }
}

impl<C> Debug for ActionsSnapshot<C> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ActionsSnapshot")
            .field("state", &self.state)
            .finish()
    }
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
                    events: action.events,
                    elapsed_secs: action.elapsed_secs,
                    fired_secs: action.fired_secs,
                }
            })
            .collect();
        Self { input_actions }
    }

    fn from_snapshot<C>(snapshot: &ActionsSnapshot<C>) -> Self {
        snapshot.state.clone()
    }

    #[allow(clippy::wrong_self_convention)]
    fn to_snapshot<C>(self) -> ActionsSnapshot<C> {
        ActionsSnapshot::<C> {
            state: self,
            _marker: core::marker::PhantomData,
        }
    }
}

impl<C: InputContext> ActionStateSequence for BEIStateSequence<C> {
    type Action = C;
    type Snapshot = ActionsSnapshot<C>;
    type State = Actions<C>;
    type Marker = InputMarker<C>;
    type Context = Res<'static, InputRegistry>;

    fn is_empty(&self) -> bool {
        self.states.is_empty()
            || self
                .states
                .iter()
                // TODO: is this correct? we could have all SameAsPrecedent but similar to something non-empty?
                .all(|s| matches!(s, InputData::Absent | InputData::SameAsPrecedent))
    }

    fn len(&self) -> usize {
        self.states.len()
    }

    fn update_buffer<'w, 's>(self, input_buffer: &mut InputBuffer<Self::Snapshot>, end_tick: Tick) {
        let start_tick = end_tick + 1 - self.len() as u16;
        // input_buffer.extend_to_range(start_tick, end_tick);
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
                        input.equals_snapshot::<C>(existing_value)
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

            if state.get_mut_by_id(kind.0).is_none() {
                // action_state is not bound, bind it
                registry.bind(*kind, state).unwrap();
            }
            // SAFETY: if the action is missing, we just bound it above
            let action_state = state.get_mut_by_id(kind.0).unwrap();

            trace!(
                "Setting action {:?} to state {:?} with value {:?}",
                kind, action.state, action.value
            );
            action_state.state = action.state;
            action_state.value = action.value;
            action_state.events = action.events;
            action_state.fired_secs = action.fired_secs;
            action_state.elapsed_secs = action.elapsed_secs;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bevy_enhanced_input::prelude::{InputAction, InputContext};
    use bevy_reflect::Reflect;
    use core::any::TypeId;

    #[derive(InputContext)]
    struct Context1;

    #[derive(InputAction, Debug, Clone, PartialEq, Eq, Hash, Reflect)]
    #[input_action(output = bool)]
    struct Action1;

    #[test]
    fn test_create_message() {
        let mut registry = InputRegistry::default();
        registry.add::<Action1>();
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
            ActionsSnapshot::<Context1> {
                state: ActionsMessage::from_actions(&action_state, &registry),
                _marker: Default::default(),
            },
        );
        let state = action_state.get_mut_by_id(type_id).unwrap();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        input_buffer.set(
            Tick(3),
            ActionsSnapshot::<Context1> {
                state: ActionsMessage::from_actions(&action_state, &registry),
                _marker: Default::default(),
            },
        );
        let state = action_state.get_mut_by_id(type_id).unwrap();
        state.state = ActionState::None;
        state.value = ActionValue::Bool(false);
        input_buffer.set(
            Tick(7),
            ActionsSnapshot::<Context1> {
                state: ActionsMessage::from_actions(&action_state, &registry),
                _marker: Default::default(),
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
                            events: ActionEvents::empty(),
                            elapsed_secs: 0.0,
                            fired_secs: 0.0,
                        }],
                    }),
                    InputData::Input(ActionsMessage {
                        input_actions: vec![InputActionMessage {
                            net_id,
                            state: ActionState::Fired,
                            value: ActionValue::Bool(true),
                            events: ActionEvents::empty(),
                            elapsed_secs: 0.0,
                            fired_secs: 0.0,
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
                            events: ActionEvents::empty(),
                            elapsed_secs: 0.0,
                            fired_secs: 0.0,
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

    #[test]
    fn test_build_from_input_buffer_empty() {
        let input_buffer: InputBuffer<ActionsSnapshot<Context1>> = InputBuffer::default();
        let sequence =
            BEIStateSequence::<Context1>::build_from_input_buffer(&input_buffer, 5, Tick(10));
        assert!(sequence.is_none());
    }

    #[test]
    fn test_build_from_input_buffer_partial_overlap() {
        let mut registry = InputRegistry::default();
        registry.add::<Action1>();
        let type_id = TypeId::of::<Action1>();
        let net_id = *registry
            .kind_map
            .net_id(&InputActionKind::from(type_id))
            .unwrap();
        let mut input_buffer = InputBuffer::default();
        let mut action_state = Actions::<Context1>::default();
        action_state.bind::<Action1>();
        input_buffer.set(
            Tick(8),
            ActionsSnapshot::<Context1> {
                state: ActionsMessage::from_actions(&action_state, &registry),
                _marker: Default::default(),
            },
        );
        let state = action_state.get_mut_by_id(type_id).unwrap();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        input_buffer.set(
            Tick(10),
            ActionsSnapshot::<Context1> {
                state: ActionsMessage::from_actions(&action_state, &registry),
                _marker: Default::default(),
            },
        );

        let sequence =
            BEIStateSequence::<Context1>::build_from_input_buffer(&input_buffer, 5, Tick(12))
                .unwrap();
        assert!(matches!(&sequence.states[0], InputData::Input(_)));
        assert_eq!(sequence.states.len(), 5);
    }

    #[test]
    fn test_update_buffer_extends_left_and_right() {
        let mut registry = InputRegistry::default();
        registry.add::<Action1>();
        let mut input_buffer = InputBuffer::default();
        let mut action_state = Actions::<Context1>::default();
        action_state.bind::<Action1>();
        let actions_msg = ActionsMessage::from_actions(&action_state, &registry);
        let sequence = BEIStateSequence::<Context1> {
            states: vec![
                InputData::Input(actions_msg.clone()),
                InputData::SameAsPrecedent,
                InputData::Absent,
            ],
            marker: Default::default(),
        };
        // This should extend the buffer to fit ticks 5..=7
        sequence.update_buffer(&mut input_buffer, Tick(7));
        assert!(input_buffer.get(Tick(5)).is_some());
        assert!(input_buffer.get(Tick(6)).is_some());
        assert!(input_buffer.get(Tick(7)).is_none());
    }
}
