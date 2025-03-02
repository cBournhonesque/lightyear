//! Handle input messages received from the clients
pub mod native;

#[cfg_attr(docsrs, doc(cfg(feature = "leafwing")))]
#[cfg(feature = "leafwing")]
pub mod leafwing;


use crate::inputs::native::input_buffer::InputBuffer;
use crate::inputs::native::UserActionState;
use crate::prelude::{server::is_started, TickManager};
use crate::shared::sets::{InternalMainSet, ServerMarker};
use bevy::prelude::*;

pub struct BaseInputPlugin<A> {
    marker: std::marker::PhantomData<A>,
}

impl<A> Default for BaseInputPlugin<A> {
    fn default() -> Self {
        Self {
            marker: std::marker::PhantomData,
        }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
    /// Receive the latest ActionDiffs from the client
    ReceiveInputs,
    /// Use the ActionDiff received from the client to update the [`ActionState`]
    UpdateActionState,
}

impl<A: UserActionState> Plugin for BaseInputPlugin<A> {
    fn build(&self, app: &mut App) {
        // SETS
        app.configure_sets(
            PreUpdate,
            (
                InternalMainSet::<ServerMarker>::ReceiveEvents,
                InputSystemSet::ReceiveInputs,
            )
                .chain()
                .run_if(is_started),
        );
        app.configure_sets(FixedPreUpdate, InputSystemSet::UpdateActionState.run_if(is_started));

        // SYSTEMS
        app.add_systems(
            FixedPreUpdate,
            update_action_state::<A>.in_set(InputSystemSet::UpdateActionState),
        );
    }
}



/// Read the InputState for the current tick from the buffer, and use them to update the ActionState
fn update_action_state<A: UserActionState>(
    tick_manager: Res<TickManager>,
    mut action_state_query: Query<(Entity, &mut A, &mut InputBuffer<A>)>,
) {
    let tick = tick_manager.tick();
    for (entity, mut action_state, mut input_buffer) in action_state_query.iter_mut() {
        // We only apply the ActionState from the buffer if we have one.
        // If we don't (because the input packet is late or lost), we won't do anything.
        // This is equivalent to considering that the player will keep playing the last action they played.
        if let Some(action) = input_buffer.get(tick) {
            *action_state = action.clone();
            trace!(?tick, ?entity, "action state after update. Input Buffer: {}", input_buffer.as_ref());
            // remove all the previous values
            // we keep the current value in the InputBuffer so that if future messages are lost, we can still
            // fallback on the last known value
            input_buffer.pop(tick - 1);

            #[cfg(feature = "metrics")]
            {
                // The size of the buffer should always bet at least 1, and hopefully be a bit more than that
                // so that we can handle lost messages
                metrics::gauge!(format!(
                    "inputs::{}::{}::buffer_size",
                    std::any::type_name::<A>(),
                    entity
                ))
                .set(input_buffer.len() as f64);
            }
        }
    }
}