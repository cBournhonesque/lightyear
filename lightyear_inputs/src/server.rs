//! Handle input messages received from the clients
use crate::input_buffer::InputBuffer;
use crate::UserActionState;
use bevy::prelude::*;
use lightyear_connection::server::Started;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_messages::plugin::MessageSet;
use tracing::trace;

pub struct BaseInputPlugin<A> {
    rebroadcast_inputs: bool,
    marker: core::marker::PhantomData<A>,
}

impl<A> Default for BaseInputPlugin<A> {
    fn default() -> Self {
        Self {
            rebroadcast_inputs: false,
            marker: core::marker::PhantomData,
        }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSet {
    /// Receive the latest ActionDiffs from the client
    ReceiveInputs,
    /// Use the ActionDiff received from the client to update the [`ActionState`]
    UpdateActionState,
    /// Rebroadcast inputs to other clients
    RebroadcastInputs,
}

impl<A: UserActionState> Plugin for BaseInputPlugin<A> {
    fn build(&self, app: &mut App) {
        // SETS
        // TODO:
        //  - could there be an issue because, client updates `state` and `fixed_update_state` and sends it to server
        //  - server only considers `state`
        //  - but host-server broadcasting their inputs only updates `state`
        app.configure_sets(
            PreUpdate, (MessageSet::Receive, InputSet::ReceiveInputs).chain()
        );
        app.configure_sets(FixedPreUpdate, InputSet::UpdateActionState);

        // for host server mode?
        #[cfg(feature = "client")]
        app.configure_sets(FixedPreUpdate, InputSet::UpdateActionState.after(
            crate::client::InputSet::BufferClientInputs
        ));

        // TODO: maybe put this in a Fixed schedule to avoid sending multiple host-server identical
        //  messages per frame if we didn't run FixedUpdate at all?
        app.configure_sets(PostUpdate, InputSet::RebroadcastInputs.before(MessageSet::Send));

        // SYSTEMS
        app.add_systems(
            FixedPreUpdate,
            update_action_state::<A>.in_set(InputSet::UpdateActionState),
        );
    }
}

/// Read the InputState for the current tick from the buffer, and use them to update the ActionState
fn update_action_state<A: UserActionState>(
    // TODO: what if there are multiple servers? we need to check on which connection we are replicating the inputs,
    //  and use the timeline from that connection?
    server: Query<&LocalTimeline, With<Started>>,
    mut action_state_query: Query<(Entity, &mut A, &mut InputBuffer<A>)>,
) {
    let Ok(timeline) = server.single() else {
        // We don't have a server timeline, so we can't update the action state
        return;
    };

    let tick = timeline.tick();
    for (entity, mut action_state, mut input_buffer) in action_state_query.iter_mut() {
        // We only apply the ActionState from the buffer if we have one.
        // If we don't (because the input packet is late or lost), we won't do anything.
        // This is equivalent to considering that the player will keep playing the last action they played.
        if let Some(action) = input_buffer.get(tick) {
            *action_state = action.clone();
            trace!(
                ?tick,
                ?entity,
                "action state after update. Input Buffer: {}",
                input_buffer.as_ref()
            );

            #[cfg(feature = "metrics")]
            {
                // The size of the buffer should always bet at least 1, and hopefully be a bit more than that
                // so that we can handle lost messages
                metrics::gauge!(format!(
                    "inputs::{}::{}::buffer_size",
                    core::any::type_name::<A>(),
                    entity
                ))
                .set(input_buffer.len() as f64);
            }
        }
        // TODO: in host-server mode, if we rebroadcast inputs, we might want to keep a bit of a history
        //  in the buffer so that we have redundancy when we broadcast to other clients
        // remove all the previous values
        // we keep the current value in the InputBuffer so that if future messages are lost, we can still
        // fallback on the last known value
        input_buffer.pop(tick - 1);
    }
}
