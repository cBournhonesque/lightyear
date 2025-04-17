#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSet {
    // PRE UPDATE
    /// Receive the InputMessage from other clients
    ReceiveInputMessages,
    // FIXED PRE UPDATE
    /// System Set where the user should emit InputEvents, they will be buffered in the InputBuffers in the BufferClientInputs set.
    /// (For Leafwing, there is nothing to do because the ActionState is updated by leafwing)
    WriteClientInputs,
    /// System Set where we update the ActionState and the InputBuffers
    /// - no rollback: we write the ActionState to the InputBuffers
    /// - rollback: we fetch the ActionState value from the InputBuffers
    BufferClientInputs,

    // FIXED POST UPDATE
    /// Prepare a message for the server with the current tick's inputs.
    /// (we do this in the FixedUpdate schedule because if the simulation is slow (e.g. 10Hz)
    /// we don't want to send an InputMessage every frame)
    PrepareInputMessage,
    /// Restore the ActionState for the correct tick (without InputDelay) from the buffer
    RestoreInputs,

    // POST UPDATE
    /// System Set to prepare the input message
    SendInputMessage,
    /// Clean up old values to prevent the buffers from growing indefinitely
    CleanUp,
}

use crate::config::InputConfig;
use crate::input_buffer::InputBuffer;
use crate::{UserAction, UserActionState};
use bevy::prelude::*;
use core::marker::PhantomData;
use lightyear_core::prelude::NetworkTimeline;
use lightyear_core::timeline::LocalTimeline;
use lightyear_messages::plugin::MessageSet;
use lightyear_sync::plugin::SyncSet;
use lightyear_sync::prelude::client::{Input, IsSynced};
use lightyear_sync::prelude::InputTimeline;
use tracing::log::kv::Source;
use tracing::trace;

pub struct BaseInputPlugin<A, F> {
    config: InputConfig<A>,
    _marker: PhantomData<F>,
}

impl<A, F> BaseInputPlugin<A, F> {
    fn new(config: InputConfig<A>) -> Self {
        Self {
            config,
            _marker: PhantomData,
        }
    }
}

impl<A, F> Default for BaseInputPlugin<A, F> {
    fn default() -> Self {
        Self::new(InputConfig::default())
    }
}


impl<A: UserActionState, F: Component> Plugin for BaseInputPlugin<A, F> {
    fn build(&self, app: &mut App) {
        // SETS
        // NOTE: this is subtle! We receive remote players messages after
        //  RunFixedMainLoopSystem::BeforeFixedMainLoop to ensure that leafwing `states` have
        //  been switched to the `fixed_update` state (see https://github.com/Leafwing-Studios/leafwing-input-manager/blob/v0.16/src/plugin.rs#L170)
        //  Conveniently, this also ensures that we run this after InternalMainSet::<ClientMarker>::ReceiveEvents
        app.configure_sets(
            RunFixedMainLoop,
            InputSet::ReceiveInputMessages
                .before(RunFixedMainLoopSystem::FixedMainLoop)
                .after(RunFixedMainLoopSystem::BeforeFixedMainLoop)
        );

        app.configure_sets(FixedPreUpdate, (InputSet::WriteClientInputs, InputSet::BufferClientInputs).chain());
        app.configure_sets(FixedPostUpdate, InputSet::PrepareInputMessage);
        app.configure_sets(
            PostUpdate,
            (
                InputSet::CleanUp,
                (
                    SyncSet::Sync,
                    // run after SyncSet to make sure that the TickEvents are handled
                    // and that the interpolation_delay injected in the message are correct
                    InputSet::SendInputMessage,
                    MessageSet::Send,
                )
                    .chain(),
            )
        );

        // SYSTEMS
        app.add_systems(FixedPreUpdate, (
                buffer_action_state::<A, F>,
                get_action_state::<A>
            )
                .chain()
                .in_set(InputSet::BufferClientInputs)
        );
        app.add_systems(
            FixedPostUpdate,
            // we want:
            // - to write diffs for the delayed tick (in the next FixedUpdate run), so re-fetch the delayed action-state
            //   this is required in case the FixedUpdate schedule runs multiple times in a frame,
            // - next frame's input-map (in PreUpdate) to act on the delayed tick, so re-fetch the delayed action-state
            get_delayed_action_state::<A, F>.in_set(InputSet::RestoreInputs),
        );
        app.add_systems(PostUpdate, clean_buffers::<A>.in_set(InputSet::CleanUp));
    }
}

/// Write the value of the ActionState in the InputBuffer.
/// (so that we can pull it for rollback or for delayed inputs)
///
/// If we have input-delay, we will store the current ActionState in the buffer at the delayed-tick,
/// and we will pull ActionStates from the buffer instead of just using the ActionState component directly.
///
/// We do not need to buffer inputs during rollback, as they have already been buffered
fn buffer_action_state<A: UserActionState, F: Component>(
    // we buffer inputs even for the Host-Server so that
    // 1. the HostServer client can broadcast inputs to other clients
    // 2. the HostServer client can have input delay
    sender: Query<(&InputTimeline, &LocalTimeline)>,
    mut action_state_query: Query<(Entity, &A, &mut InputBuffer<A>), With<F>>,
) {
    let Ok((timeline, local_timeline)) = sender.single() else {
        return;
    };
    // In rollback, we don't want to write any inputs
    if local_timeline.is_rollback() {
        return;
    }
    let current_tick = timeline.tick();
    let tick = current_tick + timeline.input_delay() as i16;
    for (entity, action_state, mut input_buffer) in action_state_query.iter_mut() {
        input_buffer.set(tick, action_state.clone());
        trace!(
            ?entity,
            action_state = ?action_state.clone(),
            ?current_tick,
            delayed_tick = ?tick,
            input_buffer = %input_buffer.as_ref(),
            "set action state in input buffer",
        );
        #[cfg(feature = "metrics")]
        {
            metrics::gauge!(format!(
                "inputs::{}::{}::buffer_size",
                core::any::type_name::<A>(),
                entity
            ))
            .set(input_buffer.len() as f64);
        }
    }
}

/// Retrieve the ActionState for the current tick.
fn get_action_state<A: UserActionState>(
    sender: Query<&LocalTimeline, With<InputTimeline>>,
    // NOTE: we want to apply the Inputs for BOTH the local player and the remote player.
    // - local player: we need to get the input from the InputBuffer because of input delay
    // - remote player: we want to reduce the amount of rollbacks by updating the ActionState
    //   as fast as possible (the inputs are broadcasted with no delay)
    mut action_state_query: Query<(Entity, &mut A, &InputBuffer<A>)>,
) {
    let Ok(local_timeline) = sender.single() else {
        return;
    };
    let input_delay = local_timeline.input_delay() as i16;
    let tick = if !local_timeline.is_rollback() {
        // If there is no rollback and no input_delay, we just buffered the input so there is nothing to do.
        if input_delay == 0 {
            return;
        }
        // If there is no rollback but input_delay, we also fetch it from the InputBuffer.
        local_timeline.tick()
    } else {
        // If there is rollback, we fetch it from the InputBuffer for the rollback tick.
        local_timeline.get_rollback_tick().unwrap()
    };

    for (entity, mut action_state, input_buffer) in action_state_query.iter_mut() {
        // We only apply the ActionState from the buffer if we have one.
        // If we don't (which could happen for remote inputs), we won't do anything.
        // This is equivalent to considering that the remote player will keep playing the last action they played.
        if let Some(action) = input_buffer.get(tick) {
            *action_state = action.clone();
            trace!(
                ?entity,
                ?tick,
                "fetched action state {:?} from input buffer: {:?}",
                action_state,
                // action_state.get_pressed(),
                input_buffer
            );
        }
    }
}


/// At the start of the frame, restore the ActionState to the latest-action state in buffer
/// (e.g. the delayed action state) because all inputs (i.e. diffs) are applied to the delayed action-state.
fn get_delayed_action_state<A: UserActionState, F: Component>(
    sender: Query<(&InputTimeline, &LocalTimeline), With<IsSynced<Input>>>,
    mut action_state_query: Query<
        (Entity, &mut A, &InputBuffer<A>),
        // Filter so that this is only for directly controlled players, not remote players
        With<F>,
    >,
) {
    let Ok((timeline, local_timeline)) = sender.single() else {
        return;
    };
    let input_delay_ticks = timeline.input_delay() as i16;
    if local_timeline.is_rollback() || input_delay_ticks == 0 {
        return;
    }
    let delayed_tick = local_timeline.tick() + input_delay_ticks;
    for (entity, mut action_state, input_buffer) in action_state_query.iter_mut() {
        // TODO: lots of clone + is complicated. Shouldn't we just have a DelayedActionState component + resource?
        //  the problem is that the Leafwing Plugin works on ActionState directly...
        if let Some(delayed_action_state) = input_buffer.get(delayed_tick) {
            *action_state = delayed_action_state.clone();
            trace!(
                ?entity,
                ?delayed_tick,
                "fetched delayed action state {:?} from input buffer: {}",
                action_state,
                input_buffer
            );
        }
        // TODO: if we don't find an ActionState in the buffer, should we reset the delayed one to default?
    }
}

/// System that removes old entries from the InputBuffer
fn clean_buffers<A: UserAction>(
    sender: Query<&LocalTimeline, With<InputTimeline>>,
    mut input_buffer_query: Query<&mut InputBuffer<A>>,
) {
    let Ok(timeline) = sender.single() else {
        return;
    };
    let old_tick = timeline.tick() - 20;

    trace!(
        "popping all input buffers since old tick: {old_tick:?}",
    );
    for mut input_buffer in input_buffer_query.iter_mut() {
        input_buffer.pop(old_tick);
    }
}
