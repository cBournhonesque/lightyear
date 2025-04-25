//! The InputBuffer contains a history of the ActionState for each tick.
//!
//! It is used for several purposes:
//! - the client's inputs for tick T must arrive before the server processes tick T, so they are stored
//!   in the buffer until the server processes them. The InputBuffer can be updated efficiently by receiving
//!   a list of [`ActionDiff`]s compared from an initial [`ActionState`]
//! - to implement input-delay, we want a button press at tick t to be processed at tick t + delay on the client.
//!   Therefore, we will store the computed ActionState at tick t + delay, but then we load the ActionState at tick t
//!   from the buffer
use bevy::platform::time::Instant;

use crate::inputs::leafwing::action_diff::ActionDiff;
use crate::shared::tick_manager::Tick;
use leafwing_input_manager::prelude::ActionState;
use tracing::trace;

use super::LeafwingUserAction;

/// The InputBuffer contains a history of the ActionState for each tick between
/// `start_tick` and `end_tick`. All ticks between `start_tick` and `end_tick` must be included in the buffer.
pub type InputBuffer<A> = crate::inputs::native::input_buffer::InputBuffer<ActionState<A>>;

impl<T: LeafwingUserAction> InputBuffer<T> {
    /// Upon receiving an [`InputMessage`](super::input_message::InputMessage), update the InputBuffer with all the inputs
    /// included in the message.
    /// TODO: disallow overwriting inputs for ticks we've already received inputs for?
    ///
    pub(crate) fn update_from_diffs(
        &mut self,
        end_tick: Tick,
        start_value: &ActionState<T>,
        diffs: &[Vec<ActionDiff<T>>],
    ) {
        let start_tick = end_tick - diffs.len() as u16;
        self.set(start_tick, start_value.clone());

        let mut value = start_value.clone();
        for (delta, diffs_for_tick) in diffs.iter().enumerate() {
            // TODO: there's an issue; we use the diffs to set future ticks after the start value, but those values
            //  have not been ticked correctly! As a workaround, we tick them manually so that JustPressed becomes Pressed,
            //  but it will NOT work for timing-related features
            value.tick(Instant::now(), Instant::now());
            let tick = start_tick + Tick(1 + delta as u16);
            for diff in diffs_for_tick {
                // TODO: also handle timings!
                diff.apply(&mut value);
            }
            self.set(tick, value.clone());
            trace!(
                "updated from input-message tick: {:?}, value: {:?}",
                tick,
                value
            );
        }
    }
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
