use crate::inputs::leafwing::action_diff::ActionDiff;
use crate::inputs::leafwing::input_buffer::InputBuffer;
use crate::prelude::{Deserialize, LeafwingUserAction, Serialize, Tick};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{Entity, EntityMapper, Reflect};
use leafwing_input_manager::action_state::ActionState;
use leafwing_input_manager::Actionlike;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
/// We serialize the inputs by sending, for each entity:
/// - the ActionState at a given start tick
/// - then the ActionDiffs for all the ticks after that, up to end_tick
///
/// (We do this to make sure that we can reconstruct the ActionState at any tick,
/// even if we miss some inputs. This wouldn't be the case if we only send ActionDiffs)
pub struct InputMessage<A: Actionlike> {
    pub(crate) end_tick: Tick,
    // first element is tick end_tick-N+1, last element is end_tick
    pub(crate) diffs: Vec<(InputTarget, ActionState<A>, Vec<Vec<ActionDiff<A>>>)>,
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

impl<A: LeafwingUserAction> MapEntities for InputMessage<A> {
    // NOTE: we do NOT map the entities for input-message because when already convert
    //  the entities on the message to the corresponding client entities when we write them
    //  in the input message

    // NOTE: we only map the inputs for the pre-predicted entities
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.diffs
            .iter_mut()
            .filter_map(|(entity, _, _)| {
                if let InputTarget::PrePredictedEntity(e) = entity {
                    return Some(e);
                } else {
                    return None;
                }
            })
            .for_each(|entity| *entity = entity_mapper.map_entity(*entity));
    }
}

impl<A: LeafwingUserAction> InputMessage<A> {
    pub fn new(end_tick: Tick) -> Self {
        Self {
            end_tick,
            diffs: vec![],
        }
    }

    /// Add the inputs for the `num_ticks` ticks starting from `self.end_tick - num_ticks + 1` up to `self.end_tick`
    ///
    /// If we don't have a starting `ActionState` from the `input_buffer`, we start from the first tick for which
    /// we have an `ActionState`.
    pub(crate) fn add_inputs(
        &mut self,
        num_ticks: u16,
        input_target: InputTarget,
        input_buffer: &InputBuffer<A>,
    ) {
        let mut inputs = Vec::new();
        // find the first tick for which we have an `ActionState` buffered
        let mut start_tick = self.end_tick - num_ticks + 1;
        while start_tick <= self.end_tick {
            if input_buffer.get(start_tick).is_some() {
                break;
            }
            start_tick += 1;
        }

        // there are no ticks for which we have an `ActionState` buffered, so we send nothing
        if start_tick > self.end_tick {
            return;
        }

        let start_value = input_buffer.get(start_tick).unwrap().clone();
        let mut tick = start_tick + 1;
        while tick <= self.end_tick {
            let diffs = ActionDiff::<A>::create(
                input_buffer.get(tick - 1).unwrap(),
                input_buffer.get(tick).unwrap(),
            );
            inputs.push(diffs);
            tick += 1;
        }
        self.diffs.push((input_target, start_value, inputs));
    }

    // TODO: do we want to send the inputs if there are no diffs?
    pub fn is_empty(&self) -> bool {
        self.diffs
            .iter()
            .all(|(_, _, diffs)| diffs.iter().all(|diffs_per_tick| diffs_per_tick.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leafwing_input_manager::Actionlike;

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
        input_message.add_inputs(5, InputTarget::Global, &input_buffer);
        assert_eq!(
            input_message,
            InputMessage {
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
