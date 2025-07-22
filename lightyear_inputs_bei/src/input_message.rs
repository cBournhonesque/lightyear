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
use lightyear_inputs::input_message::{ActionStateSequence, InputSnapshot};
use serde::{Deserialize, Serialize};
use tracing::{error, trace};

pub type SnapshotBuffer<A> = InputBuffer<ActionsSnapshot<A>>;

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
}

/// Instead of replicating the BEI Actions, we will replicate a serializable subset that can be used to
/// fully know on the remote client which actions should be triggered. This data will be used
/// to update the BEI `Actions` component
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
struct ActionsMessage {
    input_actions: Vec<InputActionMessage>,
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

impl<C: Send + Sync + 'static> InputSnapshot for ActionsSnapshot<C> {
    type Action = C;
    fn decay_tick(&mut self) {
        // TODO: maybe advance the elapsed_secs and fired_secs?
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

    fn get_snapshots_from_message(self) -> impl Iterator<Item = InputData<Self::Snapshot>> {
        self.states.into_iter().map(|input| match input {
            InputData::Absent => InputData::Absent,
            InputData::SameAsPrecedent => InputData::SameAsPrecedent,
            InputData::Input(i) => InputData::Input(i.to_snapshot()),
        })
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
    use test_log::test;
    use tracing::info;

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

    #[test]
    fn test_update_buffer_empty_buffer() {
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
        let state = action_state.get_mut_by_id(type_id).unwrap();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        let actions_msg = ActionsMessage::from_actions(&action_state, &registry);

        let sequence = BEIStateSequence::<Context1> {
            states: vec![
                InputData::Input(actions_msg.clone()),
                InputData::SameAsPrecedent,
                InputData::Absent,
            ],
            marker: Default::default(),
        };

        let earliest_mismatch = sequence.update_buffer(&mut input_buffer, Tick(7));

        info!("Input buffer after update: {:?}", input_buffer);

        // With empty buffer, the first tick received is a mismatch
        assert_eq!(earliest_mismatch, Some(Tick(5)));
        assert_eq!(input_buffer.start_tick, Some(Tick(5)));
        assert_eq!(
            input_buffer.get(Tick(5)),
            Some(&actions_msg.clone().to_snapshot())
        );
        assert_eq!(
            input_buffer.get(Tick(6)),
            Some(&actions_msg.clone().to_snapshot())
        );
        assert_eq!(input_buffer.get(Tick(7)), None);
    }

    #[test]
    fn test_update_buffer_last_action_absent_new_action_present() {
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

        // Set up buffer with an absent action at tick 5
        input_buffer.set_empty(Tick(5));

        // Create a new action for the message
        let state = action_state.get_mut_by_id(type_id).unwrap();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        let actions_msg = ActionsMessage::from_actions(&action_state, &registry);

        let sequence = BEIStateSequence::<Context1> {
            states: vec![
                InputData::Input(actions_msg.clone()),
                InputData::SameAsPrecedent,
            ],
            marker: Default::default(),
        };

        let earliest_mismatch = sequence.update_buffer(&mut input_buffer, Tick(8));

        // Should detect mismatch at tick 7 (first tick after previous_end_tick=5)
        // We predicted continuation of Absent, but got an Input
        assert_eq!(earliest_mismatch, Some(Tick(7)));
        // Filled the gap with SameAsPrecedent at tick 6, then set the new action at tick 7 and 8
        assert_eq!(input_buffer.get_raw(Tick(6)), &InputData::SameAsPrecedent);
        assert_eq!(
            input_buffer.get(Tick(7)),
            Some(&actions_msg.clone().to_snapshot())
        );
        assert_eq!(
            input_buffer.get(Tick(8)),
            Some(&actions_msg.clone().to_snapshot())
        );
    }

    #[test]
    fn test_update_buffer_last_action_present_new_action_absent() {
        let mut registry = InputRegistry::default();
        registry.add::<Action1>();
        let type_id = TypeId::of::<Action1>();
        let mut input_buffer = InputBuffer::default();
        let mut action_state = Actions::<Context1>::default();
        action_state.bind::<Action1>();

        // Set up buffer with a present action at tick 5
        let state = action_state.get_mut_by_id(type_id).unwrap();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        let actions_msg = ActionsMessage::from_actions(&action_state, &registry);
        input_buffer.set(Tick(5), actions_msg.to_snapshot());

        let sequence = BEIStateSequence::<Context1> {
            states: vec![InputData::Absent, InputData::SameAsPrecedent],
            marker: Default::default(),
        };

        let earliest_mismatch = sequence.update_buffer(&mut input_buffer, Tick(7));

        // Should detect mismatch at tick 6 (first tick after previous_end_tick=5)
        // We predicted continuation of the action, but got Absent
        assert_eq!(earliest_mismatch, Some(Tick(6)));
        assert_eq!(input_buffer.get(Tick(6)), None);
        assert_eq!(input_buffer.get(Tick(7)), None);
    }

    #[test]
    fn test_update_buffer_action_mismatch() {
        let mut registry = InputRegistry::default();
        registry.add::<Action1>();
        let type_id = TypeId::of::<Action1>();
        let mut input_buffer = InputBuffer::default();
        let mut action_state = Actions::<Context1>::default();
        action_state.bind::<Action1>();

        // Set up buffer with one action at tick 5
        let state = action_state.get_mut_by_id(type_id).unwrap();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        let first_action = ActionsMessage::from_actions(&action_state, &registry);
        input_buffer.set(Tick(5), first_action.to_snapshot());

        // Create a different action for the message
        let state = action_state.get_mut_by_id(type_id).unwrap();
        state.state = ActionState::Ongoing;
        state.value = ActionValue::Bool(false);
        let second_action = ActionsMessage::from_actions(&action_state, &registry);

        let sequence = BEIStateSequence::<Context1> {
            states: vec![
                InputData::Input(second_action.clone()),
                InputData::SameAsPrecedent,
            ],
            marker: Default::default(),
        };

        let earliest_mismatch = sequence.update_buffer(&mut input_buffer, Tick(7));

        // Should detect mismatch at tick 6 (first tick after previous_end_tick=5)
        // We predicted continuation of first_action, but got second_action
        assert_eq!(earliest_mismatch, Some(Tick(6)));
        assert_eq!(
            input_buffer.get(Tick(6)),
            Some(&second_action.clone().to_snapshot())
        );
        assert_eq!(
            input_buffer.get(Tick(7)),
            Some(&second_action.clone().to_snapshot())
        );
    }

    #[test]
    fn test_update_buffer_no_mismatch_same_action() {
        let mut registry = InputRegistry::default();
        registry.add::<Action1>();
        let type_id = TypeId::of::<Action1>();
        let mut input_buffer = InputBuffer::default();
        let mut action_state = Actions::<Context1>::default();
        action_state.bind::<Action1>();

        // Set up buffer with an action at tick 5
        let state = action_state.get_mut_by_id(type_id).unwrap();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        let actions_msg = ActionsMessage::from_actions(&action_state, &registry);
        input_buffer.set(Tick(5), actions_msg.clone().to_snapshot());

        let sequence = BEIStateSequence::<Context1> {
            states: vec![
                InputData::Input(actions_msg.clone()),
                InputData::SameAsPrecedent,
            ],
            marker: Default::default(),
        };

        let earliest_mismatch = sequence.update_buffer(&mut input_buffer, Tick(7));

        // Should be no mismatch since the action matches our prediction
        assert_eq!(earliest_mismatch, None);
        assert_eq!(input_buffer.get_raw(Tick(6)), &InputData::SameAsPrecedent);
        assert_eq!(input_buffer.get_raw(Tick(7)), &InputData::SameAsPrecedent);
        assert_eq!(input_buffer.get_raw(Tick(8)), &InputData::Absent);
    }

    #[test]
    fn test_update_buffer_skip_ticks_before_previous_end() {
        let mut registry = InputRegistry::default();
        registry.add::<Action1>();
        let type_id = TypeId::of::<Action1>();
        let mut input_buffer = InputBuffer::default();
        let mut action_state = Actions::<Context1>::default();
        action_state.bind::<Action1>();

        // Set up buffer with actions at ticks 5 and 6
        let state = action_state.get_mut_by_id(type_id).unwrap();
        state.state = ActionState::Fired;
        state.value = ActionValue::Bool(true);
        let first_action = ActionsMessage::from_actions(&action_state, &registry);
        input_buffer.set(Tick(5), first_action.clone().to_snapshot());
        input_buffer.set(Tick(6), first_action.clone().to_snapshot());

        // Create a different action
        let state = action_state.get_mut_by_id(type_id).unwrap();
        state.state = ActionState::Ongoing;
        state.value = ActionValue::Bool(false);
        let second_action = ActionsMessage::from_actions(&action_state, &registry);

        // Message covers ticks 5-8, but we should only process ticks 7-8
        let sequence = BEIStateSequence::<Context1> {
            states: vec![
                InputData::Input(first_action.clone()), // tick 5 - should be skipped
                InputData::SameAsPrecedent,             // tick 6 - should be skipped
                InputData::Input(second_action.clone()), // tick 7 - should detect mismatch
                InputData::SameAsPrecedent,             // tick 8 - should be processed
            ],
            marker: Default::default(),
        };

        let earliest_mismatch = sequence.update_buffer(&mut input_buffer, Tick(8));

        // Should detect mismatch at tick 7 (first tick after previous_end_tick=6)
        assert_eq!(earliest_mismatch, Some(Tick(7)));
        // Ticks 5 and 6 should remain unchanged
        assert_eq!(
            input_buffer.get(Tick(5)),
            Some(&first_action.clone().to_snapshot())
        );
        assert_eq!(
            input_buffer.get(Tick(6)),
            Some(&first_action.clone().to_snapshot())
        );
        // Ticks 7 and 8 should be updated
        assert_eq!(
            input_buffer.get(Tick(7)),
            Some(&second_action.clone().to_snapshot())
        );
        assert_eq!(
            input_buffer.get(Tick(8)),
            Some(&second_action.clone().to_snapshot())
        );
    }
}
