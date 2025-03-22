pub mod native;

#[cfg_attr(docsrs, doc(cfg(feature = "leafwing")))]
#[cfg(feature = "leafwing")]
pub mod leafwing;

//
// /// Returns true if there is input delay present
// pub fn is_input_delay(identity: Option<Res<State<NetworkIdentityState>>>, config: Res<ClientConfig>) -> bool {
//     // if we are running in host-server mode, disable input delay
//     // (because the InputBuffer on a given entity is shared between client and server)
//     !identity.is_some_and(|i| i.get() == &NetworkIdentityState::HostServer) &&
//     config.prediction.minimum_input_delay_ticks > 0
//         || config.prediction.maximum_input_delay_before_prediction > 0
//         || config.prediction.maximum_predicted_ticks < 30
// }
//
/// Returns true if there is input delay present
pub fn is_input_delay(config: Res<ClientConfig>) -> bool {
    config.prediction.minimum_input_delay_ticks > 0
        || config.prediction.maximum_input_delay_before_prediction > 0
        || config.prediction.maximum_predicted_ticks < 30
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
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

use bevy::prelude::*;
use core::marker::PhantomData;
use tracing::trace;

use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::prediction::plugin::is_in_rollback;
use crate::client::prediction::rollback::Rollback;
use crate::client::run_conditions::is_synced;
use crate::client::sync::SyncSet;
use crate::inputs::native::input_buffer::InputBuffer;
use crate::inputs::native::input_message::InputMessage;
use crate::inputs::native::{UserAction, UserActionState};
use crate::prelude::{is_host_server, TickManager};
use crate::shared::input::InputConfig;
use crate::shared::sets::{ClientMarker, InternalMainSet};

pub(crate) struct BaseInputPlugin<A, F> {
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

// TODO: is this actually necessary? The sync happens in PostUpdate,
//  so maybe it's ok if the InputMessages contain the pre-sync tick! (since those inputs happened
//  before the sync). If it's not needed, send the messages directly in FixedPostUpdate!
//  Actually maybe it is, because the send-tick on the server will be updated.
/// Buffer that will store the InputMessages we want to write this frame.
///
/// We need this because:
/// - we write the InputMessages during FixedPostUpdate
/// - we apply the TickUpdateEvents (from doing sync) during PostUpdate, which might affect the ticks from the InputMessages.
///   During this phase, we want to update the tick of the InputMessages that we wrote during FixedPostUpdate.
#[derive(Debug, Resource)]
struct MessageBuffer<A>(Vec<InputMessage<A>>);

impl<A: UserActionState, F: Component> Plugin for BaseInputPlugin<A, F> {
    fn build(&self, app: &mut App) {
        // in host-server mode, we don't need to handle inputs in any way, because the player's entity
        // is spawned with `InputBuffer` and the client is in the same timeline as the server
        let should_run = not(is_host_server);

        // SETS
        // NOTE: this is subtle! We receive remote players messages after
        //  RunFixedMainLoopSystem::BeforeFixedMainLoop to ensure that leafwing `states` have
        //  been switched to the `fixed_update` state (see https://github.com/Leafwing-Studios/leafwing-input-manager/blob/v0.16/src/plugin.rs#L170)
        //  Conveniently, this also ensures that we run this after InternalMainSet::<ClientMarker>::ReceiveEvents
        app.configure_sets(
            RunFixedMainLoop,
            InputSystemSet::ReceiveInputMessages
                .before(RunFixedMainLoopSystem::FixedMainLoop)
                .after(RunFixedMainLoopSystem::BeforeFixedMainLoop)
                .run_if(should_run.clone()),
        );

        app.configure_sets(
            FixedPreUpdate,
            (
                // we still need to be able to update inputs in host-server mode!
                InputSystemSet::WriteClientInputs,
                // NOTE: we could not buffer inputs in host-server mode, but it's required if
                //  we want the host-server client to broadcast its inputs to other clients!
                //  Also it's useful so that the host-server can have input-delay
                InputSystemSet::BufferClientInputs, // .run_if(should_run.clone())
            )
                .chain(),
        );
        app.configure_sets(
            FixedPostUpdate,
            InputSystemSet::PrepareInputMessage.run_if(should_run.clone().and(is_synced)),
        );
        app.configure_sets(
            PostUpdate,
            (
                SyncSet,
                // run after SyncSet to make sure that the TickEvents are handled
                // and that the interpolation_delay injected in the message are correct
                (InputSystemSet::SendInputMessage, InputSystemSet::CleanUp)
                    .chain()
                    // no need to run in host-server because the server already cleans up the buffers
                    .run_if(should_run.clone().and(is_synced)),
                InternalMainSet::<ClientMarker>::Send,
            )
                .chain(),
        );

        // SYSTEMS
        app.add_systems(
            FixedPreUpdate,
            (
                (
                    // We run this even in host-server mode because there might be input-delay.
                    // Also we want to buffer inputs in the InputBuffer so that we can broadcast
                    // the host-server client's inputs to other clients
                    buffer_action_state::<A, F>,
                    // If InputDelay is enabled, we get the ActionState for the current tick
                    // from the InputBuffer (which was added to the InputBuffer input_delay ticks ago)
                    //
                    // In host-server mode, we run the server's UpdateActionState which basically does this,
                    // but also removes old inputs from the buffer!
                    get_non_rollback_action_state::<A>
                        .run_if(is_input_delay.and(should_run.clone())),
                )
                    .chain()
                    .run_if(not(is_in_rollback)),
                get_rollback_action_state::<A>.run_if(is_in_rollback),
            )
                .in_set(InputSystemSet::BufferClientInputs),
        );
        app.add_systems(
            FixedPostUpdate,
            // TODO: think about how we can avoid this, maybe have a separate DelayedActionState component?
            // we want:
            // - to write diffs for the delayed tick (in the next FixedUpdate run), so re-fetch the delayed action-state
            //   this is required in case the FixedUpdate schedule runs multiple times in a frame,
            // - next frame's input-map (in PreUpdate) to act on the delayed tick, so re-fetch the delayed action-state
            get_delayed_action_state::<A, F>
                .run_if(is_input_delay.and(not(is_in_rollback)))
                .in_set(InputSystemSet::RestoreInputs),
        );

        app.add_systems(
            PostUpdate,
            (clean_buffers::<A>.in_set(InputSystemSet::CleanUp),),
        );
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
    config: Res<ClientConfig>,
    connection_manager: Res<ConnectionManager>,
    tick_manager: Res<TickManager>,
    mut action_state_query: Query<(Entity, &A, &mut InputBuffer<A>), With<F>>,
) {
    let input_delay_ticks = connection_manager.input_delay_ticks() as i16;
    let tick = tick_manager.tick() + input_delay_ticks;
    for (entity, action_state, mut input_buffer) in action_state_query.iter_mut() {
        input_buffer.set(tick, action_state.clone());
        trace!(
            ?entity,
            action_state = ?action_state.clone(),
            current_tick = ?tick_manager.tick(),
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

/// Retrieve the ActionState from the InputBuffer (if input_delay is enabled)
///
/// If we have input-delay, we need to set the ActionState for the current tick
/// using the value stored in the buffer (since the local ActionState is for the delayed tick)
fn get_non_rollback_action_state<A: UserActionState>(
    tick_manager: Res<TickManager>,
    // NOTE: we want to apply the Inputs for BOTH the local player and the remote player.
    // - local player: we need to get the input from the InputBuffer because of input delay
    // - remote player: we want to reduce the amount of rollbacks by updating the ActionState
    //   as fast as possible (the inputs are broadcasted with no delay)
    mut action_state_query: Query<(Entity, &mut A, &InputBuffer<A>)>,
) {
    let tick = tick_manager.tick();
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

/// During rollback, fetch the action-state from the InputBuffer for the corresponding tick and use that
/// to set the ActionState resource/component.
///
/// We are using the InputBuffer instead of the PredictedHistory because they are a bit different:
/// - the PredictedHistory is updated at PreUpdate whenever we receive a server message; but here we update every tick
///   (both for the player's inputs and for the remote player's inputs if we send them every tick)
/// - on rollback, we erase the PredictedHistory (because we are going to rollback to compute a new one), but inputs
///   are different, they shouldn't be erased or overriden since they are not generated from doing the rollback!
///
/// For actions from other players (with no InputMap), we replicate the ActionState so we have the
/// correct ActionState value at the rollback tick. To add even more precision during the rollback,
/// we can use the raw InputMessage of the remote player (broadcasted by the server).
/// We will apply those InputDiffs up to the most recent tick available, and then we leave the ActionState as is.
/// This is equivalent to considering that the remove player will keep playing the last action they played.
///
/// This is better than just using the ActionState from the rollback tick, because we have additional information (tick)
/// for the remote inputs that we can use to have a higher precision rollback.
/// TODO: implement some decay for the rollback ActionState of other players?
fn get_rollback_action_state<A: UserActionState>(
    mut player_action_state_query: Query<(Entity, &mut A, &InputBuffer<A>)>,
    rollback: Res<Rollback>,
) {
    let tick = rollback
        .get_rollback_tick()
        .expect("we should be in rollback");
    for (entity, mut action_state, input_buffer) in player_action_state_query.iter_mut() {
        *action_state = input_buffer.get(tick).cloned().unwrap_or_default();
        trace!(
            ?entity,
            ?tick,
            ?action_state,
            "updated action state for rollback using input_buffer: {}",
            input_buffer
        );
    }
}

/// At the start of the frame, restore the ActionState to the latest-action state in buffer
/// (e.g. the delayed action state) because all inputs (i.e. diffs) are applied to the delayed action-state.
fn get_delayed_action_state<A: UserActionState, F: Component>(
    config: Res<ClientConfig>,
    tick_manager: Res<TickManager>,
    connection_manager: Res<ConnectionManager>,
    mut action_state_query: Query<
        (Entity, &mut A, &InputBuffer<A>),
        // Filter so that this is only for directly controlled players, not remote players
        With<F>,
    >,
) {
    let input_delay_ticks = connection_manager.input_delay_ticks() as i16;
    let delayed_tick = tick_manager.tick() + input_delay_ticks;
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
    connection: Res<ConnectionManager>,
    tick_manager: Res<TickManager>,
    mut input_buffer_query: Query<(Entity, &mut InputBuffer<A>)>,
) {
    // delete old input values
    // anything beyond interpolation tick should be safe to be deleted
    let interpolation_tick = connection.sync_manager.interpolation_tick(&tick_manager);
    trace!(
        "popping all input buffers since interpolation tick: {:?}",
        interpolation_tick
    );
    for (entity, mut input_buffer) in input_buffer_query.iter_mut() {
        input_buffer.pop(interpolation_tick);
    }
}
