use std::collections::VecDeque;
use std::fmt::{Debug, Formatter};

use bevy::math::Vec2;
use bevy::prelude::{
    Component, Entity, EntityMapper, Event, FromReflect, Reflect, Resource, TypePath,
};
use bevy::reflect::DynamicTypePath;
use bevy::utils::HashMap;
use leafwing_input_manager::axislike::DualAxisData;
use leafwing_input_manager::prelude::ActionState;
use leafwing_input_manager::Actionlike;
use serde::{Deserialize, Serialize};
use tracing::trace;

use crate::prelude::client::SyncComponent;
use crate::prelude::{LightyearMapEntities, Message, Named};
use crate::protocol::BitSerializable;
use crate::shared::tick_manager::Tick;

use super::LeafwingUserAction;

// NOTE: we can have multiple Actionlike, (each entity could have a different Actionlike),
//  so we will have a separate InputBuffer for each!

// CLIENT:
// - store the diffs for each past ticks
// - during rollback we can apply the diffs in reverse -> is this possible?
//   - if not possible, we just store the ActionState for each tick (a bit expensive...)
//   - should be ok if we pre-allocate

// SERVER:
// - we receive a message containing for each tick a list of diffs
// - we apply the ticks on the right tick to the entity/resource
// - no need to maintain our inputbuffer on the server

impl<A: LeafwingUserAction> LightyearMapEntities for ActionState<A> {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

impl<A: LeafwingUserAction> Named for ActionState<A> {
    // const NAME: &'static str = formatcp!("ActionState<{}>", A::short_type_path());
    const NAME: &'static str = "ActionState";
    // const NAME: &'static str = Self::short_type_path();
}

// impl<A: LeafwingUserAction> SyncComponent for ActionState<A> {
//     fn mode() -> ComponentSyncMode {
//         // For client-side prediction of other clients, we need the ActionState to be synced from the Confirmed
//         // to the predicted entity
//         ComponentSyncMode::Simple
//     }
// }

// impl<A: UserAction> Message for InputMap<A> {}
// impl<'a, A: UserAction> MapEntities<'a> for InputMap<A> {
//     fn map_entities(&mut self, entity_mapper: Box<dyn EntityMapper + 'a>) {}
//     fn entities(&self) -> EntityHashSet<Entity> {
//         EntityHashSet::default()
//     }
// }
// impl<A: UserAction> Named for InputMap<A> {
//     fn name(&self) -> &'static str {
//         std::any::type_name::<InputMap<A>>()
//         // <A as TypePath>::short_type_path()
//         // const SHORT_TYPE_PATH: &'static str = <A as TypePath>::short_type_path();
//         // formatcp!("ActionState<{}>", SHORT_TYPE_PATH)
//     }
// }
//
// impl<A: UserAction> SyncComponent for InputMap<A> {
//     fn mode() -> ComponentSyncMode {
//         // TODO: change this to Simple (in case the input map changes?)
//         ComponentSyncMode::Once
//     }
// }

// NOTE: right now, for simplicity, we will send all the action-diffs for all entities in one single message.
// TODO: improve this data structure
#[derive(Resource, Component, Debug)]
pub(crate) struct InputBuffer<A: LeafwingUserAction> {
    pub(crate) start_tick: Option<Tick>,
    pub(crate) buffer: VecDeque<BufferItem<ActionState<A>>>,
}
impl<A: LeafwingUserAction> std::fmt::Display for InputBuffer<A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let ty = std::any::type_name::<InputBuffer<A>>();
        let buffer_str = self
            .buffer
            .iter()
            .map(|item| match item {
                BufferItem::Absent => "Absent".to_string(),
                BufferItem::SameAsPrecedent => "SameAsPrecedent".to_string(),
                BufferItem::Data(data) => format!("{:?}", data.get_pressed()),
            })
            .collect::<Vec<String>>()
            .join(", ");
        write!(
            f,
            "{}. Start tick: {:?}. Buffer: {:?}",
            ty, self.start_tick, buffer_str
        )
    }
}

// We use this to avoid cloning values in the buffer too much
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub(crate) enum BufferItem<T> {
    Absent,
    SameAsPrecedent,
    Data(T),
}

impl<A: LeafwingUserAction> Default for InputBuffer<A> {
    fn default() -> Self {
        Self {
            start_tick: None,
            buffer: VecDeque::new(),
        }
    }
}

/// Will store an `ActionDiff` as well as what generated it (either an Entity, or nothing if the
/// input actions are represented by a `Resource`)
///
/// These are typically accessed using the `Events<ActionDiffEvent>` resource.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Event)]
pub struct ActionDiffEvent<A: LeafwingUserAction> {
    /// If some: the entity that has the `ActionState<A>` component
    /// If none: `ActionState<A>` is a Resource, not a component
    pub owner: Option<Entity>,
    /// The `ActionDiff` that was generated
    pub action_diff: Vec<ActionDiff<A>>,
}

/// Stores presses and releases of buttons without timing information
///
/// These are typically accessed using the `Events<ActionDiffEvent>` resource.
/// Uses a minimal storage format, in order to facilitate transport over the network.
///
/// An `ActionState` can be fully reconstructed from a stream of `ActionDiff`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
pub enum ActionDiff<A: LeafwingUserAction> {
    /// The action was pressed
    Pressed {
        /// The value of the action
        action: A,
    },
    /// The action was released
    Released {
        /// The value of the action
        action: A,
    },
    /// The value of the action changed
    ValueChanged {
        /// The value of the action
        action: A,
        /// The new value of the action
        value: f32,
    },
    /// The axis pair of the action changed
    AxisPairChanged {
        /// The value of the action
        action: A,
        /// The new value of the axis
        axis_pair: Vec2,
    },
}

impl<A: LeafwingUserAction> ActionDiff<A> {
    pub(crate) fn action(&self) -> A {
        match self {
            ActionDiff::Pressed { action } => action.clone(),
            ActionDiff::Released { action } => action.clone(),
            ActionDiff::ValueChanged { action, value: _ } => action.clone(),
            ActionDiff::AxisPairChanged {
                action,
                axis_pair: _,
            } => action.clone(),
        }
    }

    /// Applies an [`ActionDiff`] (usually received over the network) to the [`ActionState`].
    ///
    /// This lets you reconstruct an [`ActionState`] from a stream of [`ActionDiff`]s
    pub(crate) fn apply(self, action_state: &mut ActionState<A>) {
        match self {
            ActionDiff::Pressed { action } => {
                action_state.press(&action);
                // Pressing will initialize the ActionData if it doesn't exist
                action_state.action_data_mut(&action).unwrap().value = 1.0;
            }
            ActionDiff::Released { action } => {
                action_state.release(&action);
                // Releasing will initialize the ActionData if it doesn't exist
                let action_data = action_state.action_data_mut(&action).unwrap();
                action_data.value = 0.;
                action_data.axis_pair = None;
            }
            ActionDiff::ValueChanged { action, value } => {
                action_state.press(&action);
                // Pressing will initialize the ActionData if it doesn't exist
                action_state.action_data_mut(&action).unwrap().value = value;
            }
            ActionDiff::AxisPairChanged { action, axis_pair } => {
                action_state.press(&action);
                // Pressing will initialize the ActionData if it doesn't exist
                let action_data = action_state.action_data_mut(&action).unwrap();
                action_data.axis_pair = Some(DualAxisData::from_xy(axis_pair));
                action_data.value = axis_pair.length();
            }
        };
    }
}

// TODO: use Mode to specify how to serialize a message (serde vs bitcode)! + can specify custom serialize function as well (similar to interpolation mode)
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
/// We serialize the inputs by sending only the ActionDiffs of the last few ticks
/// We will store the last N inputs starting from start_tick (in case of packet loss)
pub struct InputMessage<A: LeafwingUserAction> {
    pub(crate) end_tick: Tick,
    // first element is tick end_tick-N+1, last element is end_tick
    pub(crate) diffs: Vec<(InputTarget, Vec<Vec<ActionDiff<A>>>)>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Reflect)]
pub enum InputTarget {
    /// the input is for a global resource
    Global,
    /// the input is for a predicted or confirmed entity: on the client, the server's local entity is mapped to the client's confirmed entity
    Entity(Entity),
    /// the input is for a pre-predicted entity: on the server, the server's local entity is mapped to the client's pre-predicted entity
    PrePredictedEntity(Entity),
}

impl<A: LeafwingUserAction> Named for InputMessage<A> {
    // const NAME: &'static str = formatcp!("InputMessage<{}>", A::short_type_path());
    const NAME: &'static str = "InputMessage";
    // const NAME: &'static str = <Self as TypePath>::short_type_path();
}

impl<A: LeafwingUserAction> LightyearMapEntities for InputMessage<A> {
    // NOTE: we do NOT map the entities for input-message because when already convert
    //  the entities on the message to the corresponding client entities when we write them
    //  in the input message

    // NOTE: we only map the inputs for the pre-predicted entities
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.diffs
            .iter_mut()
            .filter_map(|(entity, _)| {
                if let InputTarget::PrePredictedEntity(e) = entity {
                    return Some(e);
                } else {
                    return None;
                }
            })
            .for_each(|entity| *entity = entity_mapper.map_entity(*entity));
    }
}

impl<T: LeafwingUserAction> InputMessage<T> {
    pub fn new(end_tick: Tick) -> Self {
        Self {
            end_tick,
            diffs: vec![],
        }
    }

    // we will always include
    pub fn is_empty(&self) -> bool {
        self.diffs
            .iter()
            .all(|(_, diffs)| diffs.iter().all(|diffs_per_tick| diffs_per_tick.is_empty()))
    }
}

impl<T: LeafwingUserAction> InputBuffer<T> {
    // Note: we expect this to be set every tick?
    //  i.e. there should be an ActionState for every tick, even if the action is None
    pub(crate) fn set(&mut self, tick: Tick, value: &ActionState<T>) {
        let Some(start_tick) = self.start_tick else {
            // initialize the buffer
            self.start_tick = Some(tick);
            self.buffer.push_back(BufferItem::Data(value.clone()));
            return;
        };

        // cannot set lower values than start_tick
        if tick < start_tick {
            return;
        }

        let end_tick = start_tick + (self.buffer.len() as i16 - 1);
        if tick > end_tick {
            // TODO: Think about how to fill the buffer between ticks
            //  - we want: if an input is missing, we consider that the user did the same action (RocketLeague or Overwatch GDC)

            // TODO: think about whether this is correct or not, it is correct if we always call set()
            //  with monotonically increasing ticks, which I think is the case
            //  maybe that's not correct because the timing information should be different? (i.e. I should tick the action-states myself
            //  and set them)
            // fill the ticks between end_tick and tick with a copy of the current ActionState
            for _ in 0..(tick - end_tick - 1) {
                trace!("fill ticks");
                self.buffer.push_back(BufferItem::SameAsPrecedent);
            }
            // add a new value to the buffer, which we will override below
            self.buffer.push_back(BufferItem::Absent);
        }

        // check if the value is the same as the precedent tick, in which case we compress it
        let mut same_as_precedent = false;
        if let Some(action_state) = self.get(tick - 1) {
            if action_state == value {
                same_as_precedent = true;
            }
        }

        // safety: we are guaranteed that the tick is in the buffer
        let entry = self.buffer.get_mut((tick - start_tick) as usize).unwrap();

        if same_as_precedent {
            *entry = BufferItem::SameAsPrecedent;
        } else {
            *entry = BufferItem::Data(value.clone());
        }
    }

    /// Remove all the inputs that are older than the given tick, then return the input
    /// for the given tick
    pub(crate) fn pop(&mut self, tick: Tick) -> Option<ActionState<T>> {
        let Some(start_tick) = self.start_tick else {
            return None;
        };
        if tick < start_tick {
            return None;
        }
        if tick > start_tick + (self.buffer.len() as i16 - 1) {
            // pop everything
            self.buffer = VecDeque::new();
            self.start_tick = Some(tick + 1);
            return None;
        }
        // info!(
        //     "buffer: {:?}. start_tick: {:?}, tick: {:?}",
        //     self.buffer, self.start_tick, tick
        // );

        // popped will represent the last value popped
        let mut popped = BufferItem::Absent;
        for _ in 0..(tick + 1 - start_tick) {
            // front is the oldest value
            let data = self.buffer.pop_front();
            if let Some(BufferItem::Data(value)) = data {
                popped = BufferItem::Data(value);
            }
        }
        self.start_tick = Some(tick + 1);

        // if the next value after we popped was 'SameAsPrecedent', we need to override it with an actual value
        if let Some(BufferItem::SameAsPrecedent) = self.buffer.front() {
            *self.buffer.front_mut().unwrap() = popped.clone();
        }

        if let BufferItem::Data(value) = popped {
            return Some(value);
        } else {
            return None;
        }
    }

    /// Get the ActionState for the given tick
    pub(crate) fn get(&self, tick: Tick) -> Option<&ActionState<T>> {
        let Some(start_tick) = self.start_tick else {
            return None;
        };
        if self.buffer.is_empty() {
            return None;
        }
        if tick < start_tick || tick > start_tick + (self.buffer.len() as i16 - 1) {
            return None;
        }
        let data = self.buffer.get((tick - start_tick) as usize).unwrap();
        match data {
            BufferItem::Absent => None,
            BufferItem::SameAsPrecedent => {
                // get the data from the preceding tick
                self.get(tick - 1)
            }
            BufferItem::Data(data) => Some(data),
        }
    }

    /// Get latest ActionState present in the buffer
    pub(crate) fn get_last(&self) -> Option<&ActionState<T>> {
        let Some(start_tick) = self.start_tick else {
            return None;
        };
        if self.buffer.is_empty() {
            return None;
        }
        self.get(start_tick + (self.buffer.len() as i16 - 1))
    }
}

/// The `ActionDiffBuffer` stores the ActionDiff received from the client for each tick
#[derive(Resource, Component, Debug)]
pub(crate) struct ActionDiffBuffer<A: LeafwingUserAction> {
    pub(crate) start_tick: Option<Tick>,
    buffer: VecDeque<HashMap<A, ActionDiff<A>>>,
}

impl<A: LeafwingUserAction> Default for ActionDiffBuffer<A> {
    fn default() -> Self {
        Self {
            start_tick: None,
            buffer: VecDeque::new(),
        }
    }
}

impl<A: LeafwingUserAction> ActionDiffBuffer<A> {
    pub(crate) fn end_tick(&self) -> Tick {
        self.start_tick.map_or(Tick(0), |start_tick| {
            start_tick + (self.buffer.len() as i16 - 1)
        })
    }

    /// Take the ActionDiff generated in the frame and use them to populate the buffer
    /// Note that multiple frame can use the same tick, in which case we will use the latest ActionDiff events
    /// for a given action
    pub(crate) fn set(&mut self, tick: Tick, diffs: Vec<ActionDiff<A>>) {
        let diffs = diffs
            .into_iter()
            .map(|diff| (diff.action(), diff))
            .collect();
        let Some(start_tick) = self.start_tick else {
            // initialize the buffer
            self.start_tick = Some(tick);
            self.buffer.push_back(diffs);
            return;
        };

        // cannot set lower values than start_tick
        if tick < start_tick {
            return;
        }

        let end_tick = start_tick + (self.buffer.len() as i16 - 1);
        if tick > end_tick {
            // fill the ticks between end_tick and tick with a copy of the current ActionState
            for _ in 0..(tick - end_tick - 1) {
                self.buffer.push_back(HashMap::default());
            }
            // add a new value to the buffer, which we will override below
            self.buffer.push_back(diffs);
            return;
        }
        // safety: we are guaranteed that the tick is in the buffer
        let entry = self.buffer.get_mut((tick - start_tick) as usize).unwrap();

        // we could have multiple ActionDiff events for the same entity, because the events were generated in different frames
        // in which case we want to merge them
        // TODO: should we handle when we have multiple ActionDiff that cancel each other? It should be fine
        //  since we read the ActionDiff in order, so the later one will cancel the earlier one
        entry.extend(diffs);
    }

    /// Remove all the diffs that are older than the given tick, then return the diffs
    /// for the given tick
    pub(crate) fn pop(&mut self, tick: Tick) -> Vec<ActionDiff<A>> {
        let Some(start_tick) = self.start_tick else {
            return vec![];
        };
        if tick < start_tick {
            return vec![];
        }
        if tick > start_tick + (self.buffer.len() as i16 - 1) {
            // pop everything
            self.buffer = VecDeque::new();
            self.start_tick = Some(tick + 1);
            return vec![];
        }

        for _ in 0..(tick - start_tick) {
            // front is the oldest value
            self.buffer.pop_front();
        }
        self.start_tick = Some(tick + 1);

        self.buffer
            .pop_front()
            .map(|v| v.into_values().collect())
            .unwrap_or(vec![])
    }

    /// Get the ActionState for the given tick
    pub(crate) fn get(&self, tick: Tick) -> Vec<ActionDiff<A>> {
        let Some(start_tick) = self.start_tick else {
            return vec![];
        };
        if tick < start_tick || tick > start_tick + (self.buffer.len() as i16 - 1) {
            return vec![];
        }
        self.buffer
            .get((tick - start_tick) as usize)
            .map(|v| v.values().cloned().collect())
            .unwrap_or(vec![])
    }
    pub(crate) fn update_from_message(&mut self, end_tick: Tick, diffs: Vec<Vec<ActionDiff<A>>>) {
        let message_start_tick = end_tick - diffs.len() as u16 + 1;
        if self.start_tick.is_none() {
            // initialize the buffer
            self.start_tick = Some(message_start_tick);
        };
        let start_tick = self.start_tick.unwrap();

        for (delta, diffs_for_tick) in diffs.into_iter().enumerate() {
            let tick = message_start_tick + Tick(delta as u16);
            self.set(tick, diffs_for_tick);
        }
    }

    // Convert the last N ticks up to end_tick included into a compressed message that we can send to the server
    // Return None if the last N inputs are all Absent
    pub(crate) fn add_to_message(
        &self,
        message: &mut InputMessage<A>,
        end_tick: Tick,
        num_ticks: u16,
        input_target: InputTarget,
    ) {
        let mut inputs = Vec::new();
        // start with the first value
        let start_tick = Tick(end_tick.0) - num_ticks + 1;
        for delta in 0..num_ticks {
            let tick = start_tick + Tick(delta);
            let diffs = self.get(tick);
            inputs.push(diffs);
        }
        message.diffs.push((input_target, inputs));
    }
}

// TODO: update from message

#[cfg(test)]
mod tests {
    use bevy::prelude::Reflect;

    use super::*;

    #[derive(
        Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Reflect, Actionlike,
    )]
    enum Action {
        Jump,
    }

    impl LeafwingUserAction for Action {}

    #[test]
    fn test_get_set_pop() {
        let mut input_buffer = InputBuffer::default();

        let mut a1 = ActionState::default();
        a1.press(&Action::Jump);
        a1.action_data_mut(&Action::Jump).unwrap().value = 0.0;
        let mut a2 = ActionState::default();
        a2.press(&Action::Jump);
        a1.action_data_mut(&Action::Jump).unwrap().value = 1.0;
        input_buffer.set(Tick(3), &a1);
        input_buffer.set(Tick(6), &a2);
        input_buffer.set(Tick(7), &a2);

        assert_eq!(input_buffer.start_tick, Some(Tick(3)));
        assert_eq!(input_buffer.buffer.len(), 5);

        assert_eq!(input_buffer.get(Tick(3)), Some(&a1));
        assert_eq!(input_buffer.get(Tick(4)), Some(&a1));
        assert_eq!(input_buffer.get(Tick(5)), Some(&a1));
        assert_eq!(input_buffer.get(Tick(6)), Some(&a2));
        assert_eq!(input_buffer.get(Tick(8)), None);

        assert_eq!(input_buffer.pop(Tick(4)), Some(a1.clone()));
        assert_eq!(input_buffer.start_tick, Some(Tick(5)));
        assert_eq!(input_buffer.buffer.len(), 3);

        // the oldest element has been updated from `SameAsPrecedent` to `Data`
        assert_eq!(
            input_buffer.buffer.front().unwrap(),
            &BufferItem::Data(a1.clone())
        );
        assert_eq!(input_buffer.pop(Tick(7)), Some(a2.clone()));
        assert_eq!(input_buffer.start_tick, Some(Tick(8)));
        assert_eq!(input_buffer.buffer.len(), 0);
    }

    #[test]
    fn test_create_message() {
        let mut diff_buffer = ActionDiffBuffer::default();

        diff_buffer.set(
            Tick(3),
            vec![ActionDiff::Pressed {
                action: Action::Jump,
            }],
        );
        diff_buffer.set(
            Tick(7),
            vec![ActionDiff::Released {
                action: Action::Jump,
            }],
        );

        let entity = Entity::from_raw(0);
        let end_tick = Tick(10);
        let mut message = InputMessage::<Action>::new(end_tick);

        diff_buffer.add_to_message(&mut message, end_tick, 9, InputTarget::Entity(entity));
        assert_eq!(
            message,
            InputMessage {
                end_tick: Tick(10),
                diffs: vec![(
                    InputTarget::Entity(entity),
                    vec![
                        vec![],
                        vec![ActionDiff::Pressed {
                            action: Action::Jump
                        }],
                        vec![],
                        vec![],
                        vec![],
                        vec![ActionDiff::Released {
                            action: Action::Jump
                        }],
                        vec![],
                        vec![],
                        vec![],
                    ]
                )],
            }
        );
    }

    #[test]
    fn test_update_from_message() {
        let mut diff_buffer = ActionDiffBuffer::default();

        let end_tick = Tick(20);
        let diffs = vec![
            vec![],
            vec![ActionDiff::Pressed {
                action: Action::Jump,
            }],
            vec![],
            vec![],
            vec![],
            vec![ActionDiff::Pressed {
                action: Action::Jump,
            }],
            vec![],
            vec![],
            vec![],
        ];

        diff_buffer.update_from_message(end_tick, diffs);

        assert_eq!(diff_buffer.get(Tick(20)), vec![]);
        assert_eq!(diff_buffer.get(Tick(19)), vec![]);
        assert_eq!(diff_buffer.get(Tick(18)), vec![]);
        assert_eq!(
            diff_buffer.get(Tick(17)),
            vec![ActionDiff::Pressed {
                action: Action::Jump
            }]
        );
        assert_eq!(diff_buffer.get(Tick(16)), vec![]);
        assert_eq!(diff_buffer.get(Tick(15)), vec![]);
        assert_eq!(diff_buffer.get(Tick(14)), vec![]);
        assert_eq!(
            diff_buffer.get(Tick(13)),
            vec![ActionDiff::Pressed {
                action: Action::Jump
            }]
        );
        assert_eq!(diff_buffer.get(Tick(12)), vec![]);
    }
}
