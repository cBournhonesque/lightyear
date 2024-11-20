//! Module to handle inputs that are defined using the `leafwing_input_manager` crate
//!
//! ### Adding leafwing inputs
//!
//! You first need to create Inputs that are defined using the [`leafwing_input_manager`](https://github.com/Leafwing-Studios/leafwing-input-manager) crate.
//! (see the documentation of the crate for more information)
//! In particular your inputs should implement the [`Actionlike`] trait.
//!
//! ```rust
//! use bevy::prelude::*;
//! use lightyear::prelude::*;
//! use lightyear::prelude::client::*;
//! use leafwing_input_manager::Actionlike;
//! use serde::{Deserialize, Serialize};
//! #[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
//! pub enum PlayerActions {
//!     Up,
//!     Down,
//!     Left,
//!     Right,
//! }
//!
//! fn main() {
//!   let mut app = App::new();
//!   app.add_plugins(LeafwingInputPlugin::<PlayerActions>::default());
//! }
//! ```
//!
//! ### Usage
//!
//! The networking of inputs is completely handled for you. You just need to add the `LeafwingInputPlugin` to your app.
//! Make sure that all your systems that depend on user inputs are added to the [`FixedUpdate`] [`Schedule`].
//!
//! Currently, global inputs (that are stored in a [`Resource`] instead of being attached to a specific [`Entity`] are not supported)
//!
//! There are some edge-cases to be careful of:
//! - the `leafwing_input_manager` crate handles inputs every frame, but `lightyear` needs to store and send inputs for each tick.
//!   This can cause issues if we have multiple ticks in a single frame, or multiple frames in a single tick.
//!   For instance, let's say you have a system in the `FixedUpdate` schedule that reacts on a button press when the button was `JustPressed`.
//!   If we have 2 frames with no FixedUpdate in between (because the framerate is high compared to the tickrate), then on the second frame
//!   the button won't be `JustPressed` anymore (it will simply be `Pressed`) so your system might not react correctly to it.
//!
use std::fmt::Debug;
use std::marker::PhantomData;

use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use tracing::{error, trace};

use crate::channel::builder::InputChannel;
use crate::client::components::Confirmed;
use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::prediction::plugin::{is_in_rollback, PredictionSet};
use crate::client::prediction::resource::PredictionManager;
use crate::client::prediction::rollback::Rollback;
use crate::client::prediction::Predicted;
use crate::client::run_conditions::is_synced;
use crate::client::sync::SyncSet;
use crate::inputs::leafwing::input_buffer::InputBuffer;
use crate::inputs::leafwing::input_message::InputTarget;
use crate::inputs::leafwing::LeafwingUserAction;
use crate::prelude::{
    is_host_server, ChannelKind, ChannelRegistry, InputMessage, MessageRegistry,
    ReplicateOnceComponent, TickManager,
};
use crate::protocol::message::MessageKind;
use crate::serialize::reader::Reader;
use crate::shared::replication::components::PrePredicted;
use crate::shared::sets::{ClientMarker, InternalMainSet};
use crate::shared::tick_manager::TickEvent;

// TODO: the resource should have a generic param, but not the user-facing config struct
#[derive(Debug, Copy, Clone, Resource)]
pub struct LeafwingInputConfig<A> {
    // TODO: right now the input-delay causes the client timeline to be more in the past than it should be
    //  I'm not sure if we can have different input_delay_ticks per ActionType
    // /// The amount of ticks that the player's inputs will be delayed by.
    // /// This can be useful to mitigate the amount of client-prediction
    // pub input_delay_ticks: u16,
    /// How many consecutive packets losses do we want to handle?
    /// This is used to compute the redundancy of the input messages.
    /// For instance, a value of 3 means that each input packet will contain the inputs for all the ticks
    ///  for the 3 last packets.
    // TODO: this seems unused now
    pub packet_redundancy: u16,

    // TODO: add an option where we send all diffs vs send only just-pressed diffs
    pub(crate) _marker: PhantomData<A>,
}

// TODO: is this actually necessary? The sync happens in PostUpdate,
//  so maybe it's ok if the InputMessages contain the pre-sync tick! (since those inputs happened
//  before the sync). If it's not needed, send the messages directly in FixedPostUpdate!
//  Actually maybe it is, because the send-tick on the server will be updated.
/// Buffer that will store the InputMessages we want to write this frame.
///
/// We need this because:
/// - we write the InputMessages during FixedPostUpdate
/// - we apply the TickUpdateEvents (from doing sync) during PostUpdate. During this phase,
/// we want to update the tick of the InputMessages that we wrote during FixedPostUpdate.
#[derive(Debug, Resource)]
struct MessageBuffer<A: LeafwingUserAction>(Vec<InputMessage<A>>);

impl<A: LeafwingUserAction> Default for MessageBuffer<A> {
    fn default() -> Self {
        Self(Vec::default())
    }
}

impl<A> Default for LeafwingInputConfig<A> {
    fn default() -> Self {
        LeafwingInputConfig {
            // input_delay_ticks: 0,
            packet_redundancy: 4,
            _marker: PhantomData,
        }
    }
}

/// Adds a plugin to handle inputs using the LeafwingInputManager
pub struct LeafwingInputPlugin<A> {
    config: LeafwingInputConfig<A>,
}

impl<A> LeafwingInputPlugin<A> {
    pub fn new(config: LeafwingInputConfig<A>) -> Self {
        Self { config }
    }
}

impl<A> Default for LeafwingInputPlugin<A> {
    fn default() -> Self {
        Self::new(LeafwingInputConfig::default())
    }
}

/// Returns true if there is input delay present
fn is_input_delay(config: Res<ClientConfig>) -> bool {
    config.prediction.minimum_input_delay_ticks > 0
        || config.prediction.maximum_input_delay_before_prediction > 0
        || config.prediction.maximum_predicted_ticks < 30
}

impl<A: LeafwingUserAction> Plugin for LeafwingInputPlugin<A>
// FLOW WITH INPUT DELAY
// - pre-update: run leafwing to update the current ActionState, which is the action-state for tick T + delay
// - fixed-pre-update:
//   - we write the current action-diffs to the buffer for tick T + d (for sending messages to server)
//   - we write the current action-state to the buffer for tick T + d (for rollbacks)
//   - get the action-state for tick T from the buffer
// - fixed-update:
//   - we use the action-state for tick T (that we got from the buffer)
// - fixed-post-update:
//   - we fetch the action-state for tick T + d from the buffer and set it on the ActionState
//     (so that it's ready for the next frame's PreUpdate, or for the next FixedPreUpdate)
// - update:
//   - the ActionState is not usable in Update, because we have the ActionState for tick T + d
// TODO: will need to generate diffs in FixedPreUpdate schedule once it's fixed in leafwing
{
    fn build(&self, app: &mut App) {
        // PLUGINS
        app.add_plugins(InputManagerPlugin::<A>::default());
        // RESOURCES
        app.insert_resource(self.config.clone());

        // in host-server mode, we don't need to handle inputs in any way, because the player's entity
        // is spawned with `InputBuffer` and the client is in the same timeline as the server
        let should_run = not(is_host_server);

        app.init_resource::<InputBuffer<A>>();
        app.init_resource::<MessageBuffer<A>>();

        // SETS
        app.configure_sets(
            PreUpdate,
            (
                InputSystemSet::AddBuffers
                    // TODO: these constraints are only necessary for entities controlled by other players
                    //  make a distinction between other players and local player
                    .after(PredictionSet::SpawnPrediction)
                    .before(PredictionSet::SpawnHistory),
                InputSystemSet::ReceiveInputMessages
                    .after(InternalMainSet::<ClientMarker>::EmitEvents),
            )
                .run_if(should_run.clone()),
        );
        app.configure_sets(
            FixedPreUpdate,
            InputSystemSet::BufferClientInputs.run_if(should_run.clone()),
        );
        app.configure_sets(
            FixedPostUpdate,
            InputSystemSet::PrepareInputMessage.run_if(should_run.clone().and_then(is_synced)),
        );
        app.configure_sets(
            PostUpdate,
            (
                SyncSet,
                // run after SyncSet to make sure that the TickEvents are handled
                (InputSystemSet::SendInputMessage, InputSystemSet::CleanUp)
                    .chain()
                    .run_if(should_run.clone().and_then(is_synced)),
                InternalMainSet::<ClientMarker>::Send,
            )
                .chain(),
        );

        // SYSTEMS
        app.add_systems(
            PreUpdate,
            (
                receive_remote_player_input_messages::<A>
                    .in_set(InputSystemSet::ReceiveInputMessages),
                add_action_state_buffer::<A>
                    .in_set(InputSystemSet::AddBuffers)
                    .after(PredictionSet::SpawnPrediction),
            ),
        );

        // NOTE: we do not tick the ActionState during FixedUpdate
        // This means that an ActionState can stay 'JustPressed' for multiple ticks, if we have multiple tick within a single frame.
        // You have 2 options:
        // - handle `JustPressed` actions in the Update schedule, where they can only happen once
        // - `consume` the action when you read it, so that it can only happen once

        // The ActionState that we have here is the ActionState for the current_tick.
        // It has been directly updated by the leafwing input manager using the user's inputs.
        app.add_systems(
            FixedPreUpdate,
            (
                (
                    // update_action_state_remote_players::<A>,
                    buffer_action_state::<A>,
                    // If InputDelay is enabled, we get the ActionState for the current tick
                    // from the InputBuffer (which was added to the InputBuffer input_delay ticks ago)
                    get_non_rollback_action_state::<A>.run_if(is_input_delay),
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
            (
                get_delayed_action_state::<A>.run_if(
                    is_input_delay
                        .and_then(should_run.clone())
                        .and_then(not(is_in_rollback)),
                ),
                prepare_input_message::<A>
                    .in_set(InputSystemSet::PrepareInputMessage)
                    // no need to prepare messages to send if in rollback
                    .run_if(not(is_in_rollback)),
            ),
        );

        // if the client tick is updated because of a desync, update the ticks in the input buffers
        app.add_observer(receive_tick_events::<A>);
        app.add_systems(
            PostUpdate,
            (
                send_input_messages::<A>.in_set(InputSystemSet::SendInputMessage),
                clean_buffers::<A>.in_set(InputSystemSet::CleanUp),
            ),
        );
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
    // PRE UPDATE
    /// Add any buffer (InputBuffer, ActionDiffBuffer) to newly spawned entities
    AddBuffers,
    /// Receive the InputMessage from other clients
    ReceiveInputMessages,
    // FIXED PRE UPDATE
    /// System Set where we update the ActionState and the InputBuffers
    /// - no rollback: we write the ActionState to the InputBuffers
    /// - rollback: we fetch the ActionState value from the InputBuffers
    BufferClientInputs,

    // FIXED POST UPDATE
    /// Prepare a message for the server with the current tick's inputs.
    /// (we do this in the FixedUpdate schedule because if the simulation is slow (e.g. 10Hz)
    /// we don't want to send an InputMessage every frame)
    PrepareInputMessage,

    // POST UPDATE
    /// System Set to prepare the input message
    SendInputMessage,
    /// Clean up old values to prevent the buffers from growing indefinitely
    CleanUp,
}

/// For each entity that has an action-state, insert an input buffer.
/// that will store the value of the action-state for the last few ticks
fn add_action_state_buffer<A: LeafwingUserAction>(
    mut commands: Commands,
    // player-controlled entities are the ones that have an InputMap
    player_entities: Query<
        (Entity, Has<ActionState<A>>),
        (
            Without<InputBuffer<A>>,
            Added<InputMap<A>>,
            // TODO: is this needed? should we just add when InputMap is added?
            // Or<(

            // (Added<ActionState<A>>, With<InputMap<A>>),
            // Added<InputMap<A>>,
            // )>,
        ),
    >,
    remote_entities: Query<
        Entity,
        (
            Added<ActionState<A>>,
            Without<InputBuffer<A>>,
            Without<InputMap<A>>,
        ),
    >,
) {
    // TODO: find a way to add input-buffer/action-diff-buffer only for controlled entity
    //  maybe provide the "controlled" component? or just use With<InputMap>?

    for (entity, has_action_state) in player_entities.iter() {
        trace!(?entity, "adding actions state buffer");
        commands.entity(entity).insert((
            // input buffer needed to rollback to a previous ActionState
            InputBuffer::<A>::default(),
            // make sure that the server entity has an ActionState component (if we use PrePrediction),
            // but don't replicate any updates after we replicated the initial component spawn
            ReplicateOnceComponent::<ActionState<A>>::default(),
        ));
        if !has_action_state {
            commands.entity(entity).insert(ActionState::<A>::default());
        }
    }
    for entity in remote_entities.iter() {
        trace!(?entity, "adding actions state buffer");
        commands.entity(entity).insert(
            // action-diff-buffer needed to store input diffs (that we can apply during rollback)
            InputBuffer::<A>::default(),
        );
    }
}

/// At the start of the frame, restore the ActionState to the latest-action state in buffer
/// (e.g. the delayed action state) because all inputs (i.e. diffs) are applied to the delayed action-state.
fn get_delayed_action_state<A: LeafwingUserAction>(
    config: Res<ClientConfig>,
    tick_manager: Res<TickManager>,
    connection_manager: Res<ConnectionManager>,
    // global_input_buffer: Res<InputBuffer<A>>,
    // global_action_state: Option<ResMut<ActionState<A>>>,
    mut action_state_query: Query<
        (Entity, &mut ActionState<A>, &InputBuffer<A>),
        With<InputMap<A>>,
    >,
) {
    let input_delay_ticks = config.prediction.input_delay_ticks(
        connection_manager.ping_manager.rtt(),
        config.shared.tick.tick_duration,
    ) as i16;
    let delayed_tick = tick_manager.tick() + input_delay_ticks;
    for (entity, mut action_state, input_buffer) in action_state_query.iter_mut() {
        // TODO: lots of clone + is complicated. Shouldn't we just have a DelayedActionState component + resource?
        //  the problem is that the Leafwing Plugin works on ActionState directly...
        if let Some(delayed_action_state) = input_buffer.get(delayed_tick) {
            *action_state = delayed_action_state.clone();
            // dbg!(input_buffer);
            // dbg!(delayed_tick);
            debug!(
                ?entity,
                ?delayed_tick,
                "fetched delayed action state {:?} from input buffer: {}",
                action_state.get_pressed(),
                input_buffer
            );
        }
        // TODO: if we don't find an ActionState in the buffer, should we reset the delayed one to default?
    }
    // if let Some(mut action_state) = global_action_state {
    //     *action_state = global_input_buffer.get_last().unwrap().clone();
    // }
}

/// Write the value of the ActionState in the InputBuffer.
/// (so that we can pull it for rollback or for delayed inputs)
///
/// If we have input-delay, we will store the current ActionState in the buffer at the delayed-tick,
/// and we will pull ActionStates from the buffer instead of just using the ActionState component directly.
///
/// We do not need to buffer inputs during rollback, as they have already been buffered
fn buffer_action_state<A: LeafwingUserAction>(
    config: Res<ClientConfig>,
    connection_manager: Res<ConnectionManager>,
    tick_manager: Res<TickManager>,
    // mut global_input_buffer: ResMut<InputBuffer<A>>,
    // global_action_state: Option<Res<ActionState<A>>>,
    mut action_state_query: Query<
        (Entity, &ActionState<A>, &mut InputBuffer<A>),
        With<InputMap<A>>,
    >,
) {
    // TODO: if the input delay changes, this could override a previous tick's input in the InputBuffer
    //  or leave gaps
    let input_delay_ticks = config.prediction.input_delay_ticks(
        connection_manager.ping_manager.rtt(),
        config.shared.tick.tick_duration,
    ) as i16;
    let tick = tick_manager.tick() + input_delay_ticks;
    for (entity, action_state, mut input_buffer) in action_state_query.iter_mut() {
        input_buffer.set(tick, action_state);
        // dbg!(tick, action_state);
        debug!(
            ?entity,
            current_tick = ?tick_manager.tick(),
            delayed_tick = ?tick,
            "set action state in input buffer: {}",
            input_buffer.as_ref()
        );
    }
    // if let Some(action_state) = global_action_state {
    //     global_input_buffer.set(tick, action_state.as_ref());
    // }
}

/// Retrieve the ActionState from the InputBuffer (if input_delay is enabled)
///
/// If we have input-delay, we need to set the ActionState for the current tick
/// using the value stored in the buffer (since the local ActionState is for the delayed tick)
fn get_non_rollback_action_state<A: LeafwingUserAction>(
    tick_manager: Res<TickManager>,
    // NOTE: we want to apply the Inputs for BOTH the local player and the remote player.
    // - local player: we need to get the input from the InputBuffer because of input delay
    // - remote player: we want to reduce the amount of rollbacks by updating the ActionState
    //   as fast as possible (the inputs are broadcasted with no delay)
    mut action_state_query: Query<
        (Entity, &mut ActionState<A>, &InputBuffer<A>),
        // With<InputMap<A>>,
    >,
) {
    let tick = tick_manager.tick();
    for (entity, mut action_state, input_buffer) in action_state_query.iter_mut() {
        // We only apply the ActionState from the buffer if we have one.
        // If we don't (which could happen for remote inputs), we won't do anything.
        // This is equivalent to considering that the remote player will keep playing the last action they played.
        if let Some(action) = input_buffer.get(tick) {
            *action_state = action.clone();
            debug!(
                ?entity,
                ?tick,
                "fetched action state {:?} from input buffer: {}",
                action_state.get_pressed(),
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
/// (both for the player's inputs and for the remote player's inputs if we send them every tick)
/// - on rollback, we erase the PredictedHistory (because we are going to rollback to compute a new one), but inputs
/// are different, they shouldn't be erased or overriden since they are not generated from doing the rollback!
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
fn get_rollback_action_state<A: LeafwingUserAction>(
    mut player_action_state_query: Query<
        (Entity, &mut ActionState<A>, &InputBuffer<A>),
        With<InputMap<A>>,
    >,
    mut remote_player_query: Query<
        (Entity, &mut ActionState<A>, &InputBuffer<A>),
        Without<InputMap<A>>,
    >,
    rollback: Res<Rollback>,
) {
    let tick = rollback
        .get_rollback_tick()
        .expect("we should be in rollback");
    for (entity, mut action_state, input_buffer) in player_action_state_query.iter_mut() {
        *action_state = input_buffer.get(tick).cloned().unwrap_or_default();
        debug!(
            ?entity,
            ?tick,
            pressed = ?action_state.get_pressed(),
            "updated action state for rollback using input_buffer: {}",
            input_buffer
        );
    }
    for (entity, mut action_state, input_buffer) in remote_player_query.iter_mut() {
        // TODO: should we reuse the existing ActionState as an optimization?
        *action_state = input_buffer.get(tick).cloned().unwrap_or_default();
        debug!(
            ?tick,
            ?entity,
            pressed = ?action_state.get_pressed(),
            "Update action state for rollback of remote player using input_buffer: {}",
            input_buffer
        );
    }
}

/// System that removes old entries from the ActionDiffBuffer and the InputBuffer
fn clean_buffers<A: LeafwingUserAction>(
    connection: Res<ConnectionManager>,
    tick_manager: Res<TickManager>,
    global_input_buffer: Option<ResMut<InputBuffer<A>>>,
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
    if let Some(mut input_buffer) = global_input_buffer {
        input_buffer.pop(interpolation_tick);
    }
}

/// Send a message to the server containing the ActionDiffs for the last few ticks
fn prepare_input_message<A: LeafwingUserAction>(
    connection: Res<ConnectionManager>,
    mut message_buffer: ResMut<MessageBuffer<A>>,
    channel_registry: Res<ChannelRegistry>,
    config: Res<ClientConfig>,
    input_config: Res<LeafwingInputConfig<A>>,
    tick_manager: Res<TickManager>,
    input_buffer_query: Query<
        (
            Entity,
            &InputBuffer<A>,
            Option<&Predicted>,
            Option<&PrePredicted>,
        ),
        With<InputMap<A>>,
    >,
) {
    let input_delay_ticks = config.prediction.input_delay_ticks(
        connection.ping_manager.rtt(),
        config.shared.tick.tick_duration,
    ) as i16;
    let tick = tick_manager.tick() + input_delay_ticks;
    // TODO: the number of messages should be in SharedConfig
    trace!(tick = ?tick, "prepare_input_message");
    // TODO: instead of redundancy, send ticks up to the latest yet ACK-ed input tick
    //  this means we would also want to track packet->message acks for unreliable channels as well, so we can notify
    //  this system what the latest acked input tick is?
    let input_send_interval = channel_registry
        .get_builder_from_kind(&ChannelKind::of::<InputChannel>())
        .unwrap()
        .settings
        .send_frequency;
    // we send redundant inputs, so that if a packet is lost, we can still recover
    // A redundancy of 2 means that we can recover from 1 lost packet
    let mut num_tick: u16 =
        ((input_send_interval.as_nanos() / config.shared.tick.tick_duration.as_nanos()) + 1)
            .try_into()
            .unwrap();
    num_tick = num_tick * input_config.packet_redundancy;
    let mut message = InputMessage::<A>::new(tick);
    for (entity, input_buffer, predicted, pre_predicted) in input_buffer_query.iter() {
        debug!(
            ?tick,
            ?entity,
            "Preparing input message with buffer: {:?}",
            input_buffer
        );

        // Make sure that server can read the inputs correctly
        // TODO: currently we are not sending inputs for pre-predicted entities until we receive the confirmation from the server
        //  could we find a way to do it?
        //  maybe if it's pre-predicted, we send the original entity (pre-predicted), and the server will apply the conversion
        //   on their end?
        if pre_predicted.is_some() {
            debug!(
                ?tick,
                "sending inputs for pre-predicted entity! Local client entity: {:?}", entity
            );
            // TODO: not sure if this whole pre-predicted inputs thing is worth it, because the server won't be able to
            //  to receive the inputs until it receives the pre-predicted spawn message.
            //  so all the inputs sent between pre-predicted spawn and server-receives-pre-predicted will be lost

            // TODO: I feel like pre-predicted inputs work well only for global-inputs, because then the server can know
            //  for which client the inputs were!

            // 0. the entity is pre-predicted, no need to convert the entity (the mapping will be done on the server, when
            // receiving the message. It's possible because the server received the PrePredicted entity before)
            message.add_inputs(
                num_tick,
                InputTarget::PrePredictedEntity(entity),
                input_buffer,
            );
        } else {
            // 1. if the entity is confirmed, we need to convert the entity to the server's entity
            // 2. if the entity is predicted, we need to first convert the entity to confirmed, and then from confirmed to remote
            if let Some(confirmed) = predicted.map_or(Some(entity), |p| p.confirmed_entity) {
                if let Some(server_entity) = connection
                    .replication_receiver
                    .remote_entity_map
                    .get_remote(confirmed)
                {
                    debug!("sending input for server entity: {:?}. local entity: {:?}, confirmed: {:?}", server_entity, entity, confirmed);
                    // println!(
                    //     "preparing input message using input_buffer: {}",
                    //     input_buffer
                    // );
                    message.add_inputs(num_tick, InputTarget::Entity(server_entity), input_buffer);
                }
            } else {
                // TODO: entity is not predicted or not confirmed? also need to do the conversion, no?
                debug!("not sending inputs because couldnt find server entity");
            }
        }
    }

    debug!(
        ?tick,
        ?num_tick,
        "sending input message for {:?}: {}",
        A::short_type_path(),
        message
    );
    message_buffer.0.push(message);

    // NOTE: keep the older input values in the InputBuffer! because they might be needed when we rollback for client prediction
}

/// Drain the messages from the buffer and send them to the server
fn send_input_messages<A: LeafwingUserAction>(
    mut connection: ResMut<ConnectionManager>,
    mut message_buffer: ResMut<MessageBuffer<A>>,
) {
    trace!(
        "Number of input messages to send: {:?}",
        message_buffer.0.len()
    );
    for mut message in message_buffer.0.drain(..) {
        connection
            .send_message::<InputChannel, InputMessage<A>>(&mut message)
            .unwrap_or_else(|err| {
                error!("Error while sending input message: {:?}", err);
            });
    }
}

/// In case the client tick changes suddenly, we also update the InputBuffer accordingly
fn receive_tick_events<A: LeafwingUserAction>(
    trigger: Trigger<TickEvent>,
    mut message_buffer: ResMut<MessageBuffer<A>>,
    mut global_input_buffer: Option<ResMut<InputBuffer<A>>>,
    mut input_buffer_query: Query<&mut InputBuffer<A>>,
) {
    match *trigger.event() {
        TickEvent::TickSnap { old_tick, new_tick } => {
            if let Some(ref mut global_input_buffer) = global_input_buffer {
                if let Some(start_tick) = global_input_buffer.start_tick {
                    trace!(
                        "Receive tick snap event {:?}. Updating global input buffer start_tick!",
                        trigger.event()
                    );
                    global_input_buffer.start_tick = Some(start_tick + (new_tick - old_tick));
                }
            }
            for mut input_buffer in input_buffer_query.iter_mut() {
                if let Some(start_tick) = input_buffer.start_tick {
                    input_buffer.start_tick = Some(start_tick + (new_tick - old_tick));
                    debug!(
                        "Receive tick snap event {:?}. Updating input buffer start_tick to {:?}!",
                        trigger.event(),
                        input_buffer.start_tick
                    );
                }
            }
            for message in message_buffer.0.iter_mut() {
                message.end_tick = message.end_tick + (new_tick - old_tick);
            }
        }
    }
}

/// Read the InputMessages of other clients from the server to update their InputBuffer and ActionState.
/// This is useful if we want to do client-prediction for remote players.
///
/// If the InputBuffer/ActionState is missing, we will add it.
///
/// We will apply the diffs on the Predicted entity.
fn receive_remote_player_input_messages<A: LeafwingUserAction>(
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    mut connection: ResMut<ConnectionManager>,
    prediction_manager: Res<PredictionManager>,
    message_registry: Res<MessageRegistry>,
    // TODO: currently we do not handle entities that are controlled by multiple clients
    confirmed_query: Query<&Confirmed, Without<InputMap<A>>>,
    mut predicted_query: Query<
        Option<&mut InputBuffer<A>>,
        (Without<InputMap<A>>, With<Predicted>),
    >,
) {
    let current_tick = tick_manager.tick();
    let kind = MessageKind::of::<InputMessage<A>>();
    let Some(net) = message_registry.kind_map.net_id(&kind).copied() else {
        error!(
            "Could not find the network id for the message kind: {:?}",
            kind
        );
        return;
    };

    if let Some(message_list) = connection.received_leafwing_input_messages.remove(&net) {
        for message_bytes in message_list {
            let mut reader = Reader::from(message_bytes);
            match message_registry.deserialize::<InputMessage<A>>(
                &mut reader,
                &mut connection
                    .replication_receiver
                    .remote_entity_map
                    .remote_to_local,
            ) {
                Ok(message) => {
                    debug!(action = ?A::short_type_path(), ?message.end_tick, ?message.diffs, "received input message");
                    for (target, start, diffs) in &message.diffs {
                        // - the input target has already been set to the server entity in the InputMessage
                        // - it has been mapped to a client-entity on the client during deserialization
                        //   ONLY if it's PrePredicted (look at the MapEntities implementation)
                        let entity = match target {
                            InputTarget::Entity(entity) => {
                                // TODO: find a better way!
                                // if InputTarget = Entity, we still need to do the mapping
                                connection
                                    .replication_receiver
                                    .remote_entity_map
                                    .get_local(*entity)
                            }
                            InputTarget::PrePredictedEntity(entity) => Some(*entity),
                            InputTarget::Global => continue,
                        };
                        if let Some(entity) = entity {
                            debug!(
                                "received input message for entity: {:?}. Applying to diff buffer.",
                                entity
                            );
                            if let Ok(confirmed) = confirmed_query.get(entity) {
                                if let Some(predicted) = confirmed.predicted {
                                    if let Ok(input_buffer) = predicted_query.get_mut(predicted) {
                                        debug!(?entity, ?diffs, end_tick = ?message.end_tick, "update action diff buffer for remote player PREDICTED using input message");
                                        if let Some(mut input_buffer) = input_buffer {
                                            input_buffer.update_from_message(
                                                message.end_tick,
                                                start,
                                                diffs,
                                            );
                                        } else {
                                            // add the ActionState or InputBuffer if they are missing
                                            let mut input_buffer = InputBuffer::<A>::default();
                                            input_buffer.update_from_message(
                                                message.end_tick,
                                                start,
                                                diffs,
                                            );
                                            // if the remote_player's predicted entity doesn't have the InputBuffer, we need to insert them
                                            commands.entity(predicted).insert((
                                                input_buffer,
                                                ActionState::<A>::default(),
                                            ));
                                        }
                                    }
                                }
                            } else {
                                error!(?entity, ?diffs, end_tick = ?message.end_tick, "received input message for unrecognized entity");
                            }
                        } else {
                            error!("received remote player input message for unrecognized entity");
                        }
                    }
                }
                Err(e) => {
                    error!(?e, "could not deserialize leafwing input message");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leafwing_input_manager::action_state::ActionState;
    use leafwing_input_manager::input_map::InputMap;
    use std::time::Duration;

    use crate::prelude::client::PredictionConfig;
    use crate::prelude::server::Replicate;
    use crate::prelude::{client, SharedConfig, TickConfig};
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;

    fn build_stepper_with_input_delay(delay_ticks: u16) -> BevyStepper {
        let frame_duration = Duration::from_millis(10);
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..default()
        };
        let client_config = ClientConfig {
            prediction: PredictionConfig {
                minimum_input_delay_ticks: delay_ticks,
                maximum_input_delay_before_prediction: 0,
                maximum_predicted_ticks: 30,
                ..default()
            },
            ..default()
        };
        let mut stepper = BevyStepper::new(shared_config, client_config, frame_duration);
        stepper.init();
        stepper
    }

    fn setup(stepper: &mut BevyStepper) -> (Entity, Entity) {
        // create an entity on server
        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn((
                ActionState::<LeafwingInput1>::default(),
                Replicate::default(),
            ))
            .id();
        // we need to step twice because we run client before server
        stepper.frame_step();
        stepper.frame_step();

        // check that the server entity got a InputBuffer added to it
        assert!(stepper
            .server_app
            .world()
            .entity(server_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .is_some());

        // check that the entity is replicated, including the ActionState component
        let client_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();
        stepper
            .client_app
            .world_mut()
            .entity_mut(client_entity)
            .insert(InputMap::<LeafwingInput1>::new([(
                LeafwingInput1::Jump,
                KeyCode::KeyA,
            )]));
        assert!(stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<ActionState<LeafwingInput1>>()
            .is_some());
        stepper.frame_step();
        (server_entity, client_entity)
    }

    /// Check that ActionStates are stored correctly in the InputBuffer
    // TODO: for the test to work correctly, I need to inspect the state during FixedUpdate schedule!
    //  otherwise the test gives me the input values outside of FixedUpdate, which is not what I want...
    //  disable the test for now until we figure it out
    #[ignore]
    #[test]
    fn test_buffer_inputs_no_delay() {
        let mut stepper = BevyStepper::default();
        let (server_entity, client_entity) = setup(&mut stepper);

        // press on a key
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyA);
        stepper.frame_step();
        let client_tick = stepper.client_tick();
        let input_buffer = stepper
            .client_app
            .world_mut()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap();
        // check that the action state got buffered
        // (we cannot use JustPressed because we start by ticking the ActionState)
        assert_eq!(
            input_buffer.get(client_tick).unwrap().get_pressed(),
            &[LeafwingInput1::Jump]
        );

        // test with another frame
        stepper.frame_step();
        let input_buffer = stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap();
        assert_eq!(
            input_buffer.get(client_tick + 1).unwrap().get_pressed(),
            &[LeafwingInput1::Jump]
        );

        // try releasing the key
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyA);
        stepper.frame_step();
        let input_buffer = stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap();
        assert!(input_buffer
            .get(client_tick + 2)
            .unwrap()
            .get_pressed()
            .is_empty());
    }

    /// Check that ActionStates are stored correctly in the InputBuffer
    // TODO: for the test to work correctly, I need to inspect the state during FixedUpdate schedule!
    //  otherwise the test gives me the input values outside of FixedUpdate, which is not what I want...
    //  disable the test for now until we figure it out
    #[ignore]
    #[test]
    fn test_buffer_inputs_with_delay() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::INFO)
        //     .init();
        let mut stepper = build_stepper_with_input_delay(1);
        let (server_entity, client_entity) = setup(&mut stepper);

        // press on a key
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyA);
        // info!("PRESS KEY");
        stepper.frame_step();
        let client_tick = stepper.client_tick();

        // check that the action state got buffered without any press (because the input is delayed)
        // (we cannot use JustPressed because we start by ticking the ActionState)
        // (i.e. the InputBuffer is empty for the current tick, and has the button press only with 1 tick of delay)
        assert!(stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap()
            .get(client_tick)
            .unwrap()
            .get_pressed()
            .is_empty());
        // outside of the FixedUpdate schedule, the ActionState should be the delayed action
        assert!(stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<ActionState<LeafwingInput1>>()
            .unwrap()
            .pressed(&LeafwingInput1::Jump));

        // release the key
        // info!("RELEASE KEY");
        stepper
            .client_app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyA);
        // step another frame, this time we get the buffered input from earlier
        stepper.frame_step();
        let input_buffer = stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap();
        assert_eq!(
            input_buffer.get(client_tick + 1).unwrap().get_pressed(),
            &[LeafwingInput1::Jump]
        );
        // the ActionState outside of FixedUpdate is the delayed one
        assert!(stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<ActionState<LeafwingInput1>>()
            .unwrap()
            .get_pressed()
            .is_empty());

        stepper.frame_step();

        assert!(stepper
            .client_app
            .world()
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap()
            .get(client_tick + 2)
            .unwrap()
            .get_pressed()
            .is_empty());
    }
}
