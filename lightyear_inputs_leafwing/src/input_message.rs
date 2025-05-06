use crate::action_diff::ActionDiff;
use crate::action_state::LeafwingUserAction;
#[cfg(not(feature = "std"))]
use alloc::{format, string::String, vec, vec::Vec};
use bevy::ecs::entity::MapEntities;
use bevy::platform::time::Instant;
use bevy::prelude::{Entity, EntityMapper, Reflect};
use core::fmt::Write;
use leafwing_input_manager::action_state::ActionState;
use leafwing_input_manager::input_map::InputMap;
use leafwing_input_manager::Actionlike;
use lightyear_core::prelude::Tick;
use lightyear_inputs::input_buffer::{InputBuffer, InputData};
use lightyear_inputs::input_message::ActionStateSequence;
use serde::{Deserialize, Serialize};
use tracing::trace;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeafwingSequence<A: Actionlike> {
    pub(crate) start_state: ActionState<A>,
    pub(crate) diffs: Vec<Vec<ActionDiff<A>>>,
}

impl<A: LeafwingUserAction> MapEntities for LeafwingSequence<A> {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

impl<A: LeafwingUserAction> ActionStateSequence for LeafwingSequence<A> {
    type Action = A;
    type State = ActionState<A>;
    type Marker = InputMap<A>;

    fn is_empty(&self) -> bool {
        self.diffs.iter().all(|diffs_per_tick| {
            diffs_per_tick.is_empty()
        })
    }

    fn len(&self) -> usize {
        self.diffs.len()
    }

    fn update_buffer(&self, input_buffer: &mut InputBuffer<Self::State>, end_tick: Tick) {
        let start_tick = end_tick - self.len() as u16;
        input_buffer.set(start_tick, self.start_state.clone());

        let mut value = self.start_state.clone();
        for (delta, diffs_for_tick) in self.diffs.iter().enumerate() {
            // TODO: there's an issue; we use the diffs to set future ticks after the start value, but those values
            //  have not been ticked correctly! As a workaround, we tick them manually so that JustPressed becomes Pressed,
            //  but it will NOT work for timing-related features
            value.tick(Instant::now(), Instant::now());
            let tick = start_tick + Tick(1 + delta as u16);
            for diff in diffs_for_tick {
                // TODO: also handle timings!
                diff.apply(&mut value);
            }
            input_buffer.set(tick, value.clone());
            trace!(
                "updated from input-message tick: {:?}, value: {:?}",
                tick,
                value
            );
        }
    }


    /// Add the inputs for the `num_ticks` ticks starting from `self.end_tick - num_ticks + 1` up to `self.end_tick`
    ///
    /// If we don't have a starting `ActionState` from the `input_buffer`, we start from the first tick for which
    /// we have an `ActionState`.
    fn build_from_input_buffer(input_buffer: &InputBuffer<Self::State>, num_ticks: u16, end_tick: Tick) -> Option<Self> {
        let mut diffs = Vec::new();
        // find the first tick for which we have an `ActionState` buffered
        let mut start_tick = end_tick - num_ticks + 1;
        while start_tick <= end_tick {
            if input_buffer.get(start_tick).is_some() {
                break;
            }
            start_tick += 1;
        }

        // there are no ticks for which we have an `ActionState` buffered, so we send nothing
        if start_tick > end_tick {
            return None;
        }

        let start_state = input_buffer.get(start_tick).unwrap().clone();
        let mut tick = start_tick + 1;
        while tick <= end_tick {
            let diffs_for_tick = ActionDiff::<A>::create(
                // TODO: if the input_delay changes, this could leave gaps in the InputBuffer, which we will fill with Default
                input_buffer
                    .get(tick - 1)
                    .unwrap_or(&ActionState::<A>::default()),
                input_buffer
                    .get(tick)
                    .unwrap_or(&ActionState::<A>::default()),
            );
            diffs.push(diffs_for_tick);
            tick += 1;
        }
        Some(Self {
            start_state,
            diffs,
        })
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use leafwing_input_manager::Actionlike;
    use lightyear_inputs::input_message::{InputMessage, InputTarget};
    use serde::{Deserialize, Serialize};

    #[derive(
        Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Reflect, Actionlike,
    )]
    enum Action {
        Jump,
    }

    #[test]
    fn test_generate_input_message_no_start_input() {
        let input_buffer = InputBuffer::default();
        let mut input_message = InputMessage::<Action>::new(Tick(10));
        input_message.add_inputs(5, InputTarget::Entity(Entity::PLACEHOLDER), &input_buffer);
        assert_eq!(
            input_message,
            InputMessage {
                interpolation_delay: None,
                end_tick: Tick(10),
                diffs: vec![],
            }
        );
        assert!(input_message.is_empty());
    }

    // #[test]
    // fn test_create_message() {
    //     let mut input_buffer = InputBuffer::default();
    //     let mut action_state = ActionState::default();
    //     input_buffer.set(Tick(2), &ActionState::default());
    //     action_state.press(&Action::Jump);
    //     input_buffer.set(Tick(3), &action_state);
    //     action_state.release(&Action::Jump);
    //
    //     diff_buffer.set(
    //         Tick(3),
    //         &vec![ActionDiff::Pressed {
    //             action: Action::Jump,
    //         }],
    //     );
    //     diff_buffer.set(
    //         Tick(7),
    //         &vec![ActionDiff::Released {
    //             action: Action::Jump,
    //         }],
    //     );
    //
    //     let entity = Entity::from_raw(0);
    //     let end_tick = Tick(10);
    //     let mut message = InputMessage::<Action>::new(end_tick);
    //
    //     message.add_inputs(9, InputTarget::Entity(entity), &diff_buffer, &input_buffer);
    //     assert_eq!(
    //         message,
    //         InputMessage {
    //             end_tick: Tick(10),
    //             diffs: vec![(
    //                 InputTarget::Entity(entity),
    //                 ActionState::default(),
    //                 vec![
    //                     vec![ActionDiff::Pressed {
    //                         action: Action::Jump
    //                     }],
    //                     vec![],
    //                     vec![],
    //                     vec![],
    //                     vec![ActionDiff::Released {
    //                         action: Action::Jump
    //                     }],
    //                     vec![],
    //                     vec![],
    //                     vec![],
    //                 ]
    //             )],
    //         }
    //     );
    // }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::inputs::native::input_buffer::InputData;
    use bevy::prelude::Reflect;
    use leafwing_input_manager::Actionlike;
    use serde::{Deserialize, Serialize};

    #[derive(
        Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Reflect, Actionlike,
    )]
    enum Action {
        Jump,
    }

    #[test]
    fn test_get_set_pop() {
        let mut input_buffer = InputBuffer::default();

        let mut a1 = ActionState::default();
        a1.press(&Action::Jump);
        let mut a2 = ActionState::default();
        a2.press(&Action::Jump);
        input_buffer.set(Tick(3), a1.clone());
        input_buffer.set(Tick(6), a2.clone());
        input_buffer.set(Tick(7), a2.clone());

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
            &InputData::Input(a1.clone())
        );
        assert_eq!(input_buffer.pop(Tick(7)), Some(a2.clone()));
        assert_eq!(input_buffer.start_tick, Some(Tick(8)));
        assert_eq!(input_buffer.buffer.len(), 0);
    }
}
