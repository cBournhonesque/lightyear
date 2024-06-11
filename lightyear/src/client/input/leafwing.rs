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
use crate::client::sync::{client_is_synced, SyncSet};
use crate::inputs::leafwing::input_buffer::InputBuffer;
use crate::inputs::leafwing::input_message::InputTarget;
use crate::inputs::leafwing::LeafwingUserAction;
use crate::prelude::{
    is_host_server, ChannelKind, ChannelRegistry, InputMessage, MessageRegistry, TickManager,
};
use crate::protocol::message::MessageKind;
use crate::serialize::reader::Reader;
use crate::shared::replication::components::PrePredicted;
use crate::shared::sets::{ClientMarker, InternalMainSet};
use crate::shared::tick_manager::TickEvent;

/// Run condition to control most of the systems in the LeafwingInputPlugin
fn run_if_enabled<A: LeafwingUserAction>(config: Res<ToggleActions<A>>) -> bool {
    config.enabled
}

#[derive(Resource)]
pub struct ToggleActions<A> {
    /// When this is false, [`ActionState`]'s corresponding to `A` will ignore user inputs
    ///
    /// When this is set to false, all corresponding [`ActionState`]s are released
    pub enabled: bool,
    /// Marker that stores the type of action to toggle
    pub phantom: PhantomData<A>,
}

// implement manually to not required the `Default` bound on A
impl<A> Default for ToggleActions<A> {
    fn default() -> Self {
        Self {
            enabled: true,
            phantom: PhantomData,
        }
    }
}

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
    config.prediction.input_delay_ticks > 0
}

impl<A: LeafwingUserAction + TypePath> Plugin for LeafwingInputPlugin<A>
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
        app.init_resource::<ToggleActions<A>>();

        // in host-server mode, we don't need to handle inputs in any way, because the player's entity
        // is spawned with `InputBuffer` and the client is in the same timeline as the server
        let should_run = run_if_enabled::<A>.and_then(not(is_host_server));

        app.init_resource::<InputBuffer<A>>();
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
            PostUpdate,
            (
                SyncSet,
                // handle tick events from sync before sending the message
                (
                    InputSystemSet::ReceiveTickEvents,
                    InputSystemSet::SendInputMessage,
                    InputSystemSet::CleanUp,
                )
                    .chain()
                    .run_if(should_run.clone().and_then(client_is_synced)),
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
                    .run_if(run_if_enabled::<A>.and_then(not(is_in_rollback))),
                get_rollback_action_state::<A>.run_if(run_if_enabled::<A>.and_then(is_in_rollback)),
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
            get_delayed_action_state::<A>.run_if(
                is_input_delay
                    .and_then(should_run.clone())
                    .and_then(not(is_in_rollback)),
            ),
        );

        // NOTE: we run the buffer_action_state system in the Update schedule for several reasons:
        // - if the fixed update schedule is too slow, we still want to have the correct input values added to the buffer
        //   for example if I have F1 TA F2 F3 TB, and I get a new button press on F2; then I want
        //   The value won't be marked as 'JustPressed' anymore on F3, so what we need to do is ...
        //   WARNING: actually we don't want to buffer here, else we would override the previous value!
        // - if the fixed update schedule is too fast, the ActionState doesn't change between the different ticks,
        //   so setting the value once at the end of the frame is enough
        //   for example if I have F1 TA F2 TB TC F3, we set the value after TA and after TC
        //   'set' will apply SameAsPrecedent for TB.
        app.add_systems(
            PostUpdate,
            (
                // NOTE:
                // - one thing to understand is that if we have F1 FU1 ( frame 1 starts, and then we run one FixedUpdate schedule)
                //   we want to add the input value computed during F1 to the buffer for tick FU1, because the tick will use this value
                prepare_input_message::<A>.in_set(InputSystemSet::SendInputMessage),
                receive_tick_events::<A>.in_set(InputSystemSet::ReceiveTickEvents),
                clean_buffers::<A>.in_set(InputSystemSet::CleanUp),
                // TODO: why is this here?
                add_action_state_buffer_added_input_map::<A>.run_if(should_run.clone()),
                toggle_actions::<A>,
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
    // FIXED UPDATE
    /// System Set where we update the ActionState and the InputBuffers
    /// - no rollback: we write the ActionState to the InputBuffers
    /// - rollback: we fetch the ActionState value from the InputBuffers
    BufferClientInputs,

    // POST UPDATE
    /// In case we suddenly changed the ticks during sync, we need to update out input buffers to the new ticks
    ReceiveTickEvents,
    /// System Set to prepare the input message
    SendInputMessage,
    /// Clean up old values to prevent the buffers from growing indefinitely
    CleanUp,
}

/// Add an [`InputBuffer`] and a [`ActionDiffBuffer`] to newly controlled entities
fn add_action_state_buffer_added_input_map<A: LeafwingUserAction>(
    mut commands: Commands,
    entities: Query<
        Entity,
        (
            With<ActionState<A>>,
            Added<InputMap<A>>,
            Without<InputBuffer<A>>,
        ),
    >,
) {
    // TODO: find a way to add input-buffer/action-diff-buffer only for controlled entity
    //  maybe provide the "controlled" component? or just use With<InputMap>?

    for entity in entities.iter() {
        debug!("added action state buffer");
        commands.entity(entity).insert(InputBuffer::<A>::default());
    }
}

/// Propagate toggle actions to the underlying leafwing plugin
fn toggle_actions<A: LeafwingUserAction>(
    config: Res<ToggleActions<A>>,
    mut leafwing_config: ResMut<leafwing_input_manager::prelude::ToggleActions<A>>,
) {
    if config.is_changed() {
        leafwing_config.enabled = config.enabled;
    }
}

/// For each entity that has an action-state, insert an action-state-buffer
/// that will store the value of the action-state for the last few ticks
fn add_action_state_buffer<A: LeafwingUserAction>(
    mut commands: Commands,
    // player-controlled entities are the ones that have an InputMap
    player_entities: Query<
        Entity,
        (
            Added<ActionState<A>>,
            Without<InputBuffer<A>>,
            With<InputMap<A>>,
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

    for entity in player_entities.iter() {
        trace!(?entity, "adding actions state buffer");
        commands.entity(entity).insert((
            // input buffer needed to rollback to a previous ActionState
            InputBuffer::<A>::default(),
        ));
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
    tick_manager: Res<TickManager>,
    global_input_buffer: Res<InputBuffer<A>>,
    global_action_state: Option<ResMut<ActionState<A>>>,
    mut action_state_query: Query<
        (Entity, &mut ActionState<A>, &InputBuffer<A>),
        With<InputMap<A>>,
    >,
) {
    for (entity, mut action_state, input_buffer) in action_state_query.iter_mut() {
        // TODO: lots of clone + is complicated. Shouldn't we just have a DelayedActionState component + resource?
        //  the problem is that the Leafwing Plugin works on ActionState directly...
        *action_state = input_buffer
            .get_last()
            .unwrap_or(&ActionState::<A>::default())
            .clone();
        let end_tick = input_buffer.end_tick();
        error!(current_tick = ?tick_manager.tick(), ?end_tick, "restored delayed action state: {:?}", action_state.get_pressed());
    }
    if let Some(mut action_state) = global_action_state {
        *action_state = global_input_buffer.get_last().unwrap().clone();
    }
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
    tick_manager: Res<TickManager>,
    // mut global_input_buffer: ResMut<InputBuffer<A>>,
    // global_action_state: Option<Res<ActionState<A>>>,
    mut action_state_query: Query<
        (Entity, &ActionState<A>, &mut InputBuffer<A>),
        With<InputMap<A>>,
    >,
) {
    let input_delay_ticks = config.prediction.input_delay_ticks as i16;
    let tick = tick_manager.tick() + input_delay_ticks;
    for (entity, action_state, mut input_buffer) in action_state_query.iter_mut() {
        trace!(
            ?entity,
            ?tick,
            delay = ?input_delay_ticks,
            "ACTION_STATE: JUST PRESSED: {:?}/ JUST RELEASED: {:?}/ PRESSED: {:?}/ RELEASED: {:?}",
            action_state.get_just_pressed(),
            action_state.get_just_released(),
            action_state.get_pressed(),
            action_state.get_released(),
        );
        trace!(?entity, ?tick, "set action state in input buffer");
        input_buffer.set(tick, action_state);
        error!(
            ?entity,
            current_tick = ?tick_manager.tick(),
            buffer_tick = ?tick,
            "set action state in input buffer: {}",
            input_buffer.as_ref()
        );
        trace!(
            ?entity,
            ?tick,
            "input buffer. Start tick {:?}, len: {:?}",
            input_buffer.start_tick,
            input_buffer.buffer.len()
        );
    }
    // if let Some(action_state) = global_action_state {
    //     global_input_buffer.set(tick, action_state.as_ref());
    // }
}

/// Retrieve the ActionState from the InputBuffer (if input_delay is enabled)
// TODO: combine this with the rollback function
// If we have input-delay, we need to set the ActionState for the current tick
// using the value stored in the buffer
fn get_non_rollback_action_state<A: LeafwingUserAction>(
    tick_manager: Res<TickManager>,
    // global_input_buffer: Res<InputBuffer<A>>,
    // global_action_state: Option<ResMut<ActionState<A>>>,
    mut action_state_query: Query<
        (Entity, &mut ActionState<A>, &InputBuffer<A>),
        With<InputMap<A>>,
    >,
) {
    let tick = tick_manager.tick();
    for (entity, mut action_state, input_buffer) in action_state_query.iter_mut() {
        // let state_is_empty = input_buffer.get(tick).is_none();
        // let input_buffer = input_buffer.buffer;
        // error!(?entity, ?tick, "get action state. Buffer: {}", input_buffer);
        *action_state = input_buffer
            .get(tick)
            .unwrap_or(&ActionState::<A>::default())
            .clone();
        error!(
            ?entity,
            ?tick,
            "fetched action state {:?} from input buffer: {}",
            action_state.get_pressed(),
            input_buffer
        );
    }
    // if let Some(mut action_state) = global_action_state {
    //     *action_state = global_input_buffer
    //         .get(tick)
    //         .unwrap_or(&ActionState::<A>::default())
    //         .clone();
    // }
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
    // global_input_buffer: Res<InputBuffer<A>>,
    // global_action_state: Option<ResMut<ActionState<A>>>,
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
        trace!(
            ?entity,
            ?tick,
            "get rollback action state. Buffer: {}",
            input_buffer
        );
        *action_state = input_buffer.get(tick).cloned().unwrap_or_default();
        error!(
            ?entity,
            ?tick,
            pressed = ?action_state.get_pressed(),
            "updated action state for rollback using input_buffer: {}",
            input_buffer
        );
    }
    for (entity, mut action_state, input_buffer) in remote_player_query.iter_mut() {
        error!(
            ?tick,
            ?entity,
            ?input_buffer,
            "action state: {:?}. Latest action diff buffer tick: {:?}",
            &action_state.get_pressed(),
            input_buffer.end_tick(),
        );
        // TODO: should we reuse the existing ActionState as an optimization?
        *action_state = input_buffer.get(tick).cloned().unwrap_or_default();
    }
    // if let Some(mut action_state) = global_action_state {
    //     *action_state = global_input_buffer.get(tick).cloned().unwrap_or_default();
    // }
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
    mut connection: ResMut<ConnectionManager>,
    channel_registry: Res<ChannelRegistry>,
    config: Res<ClientConfig>,
    input_config: Res<LeafwingInputConfig<A>>,
    tick_manager: Res<TickManager>,
    // global_action_diff_buffer: Option<Res<ActionDiffBuffer<A>>>,
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
    let tick = tick_manager.tick() + config.prediction.input_delay_ticks as i16;
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
                    .copied()
                {
                    debug!("sending input for server entity: {:?}. local entity: {:?}, confirmed: {:?}", server_entity, entity, confirmed);
                    message.add_inputs(num_tick, InputTarget::Entity(server_entity), input_buffer);
                }
            } else {
                // TODO: entity is not predicted or not confirmed? also need to do the conversion, no?
                debug!("not sending inputs because couldnt find server entity");
            }
        }
    }

    // if let Some(action_diff_buffer) = global_action_diff_buffer {
    //     action_diff_buffer.add_to_message(&mut message, tick, message_len, InputTarget::Global);
    // }

    // all inputs are absent
    // TODO: should we provide variants of each user-facing function, so that it pushes the error
    //  to the ConnectionEvents?
    // if !message.is_empty() {
    error!(?tick, ?num_tick, "sending input message: {}", message);
    connection
        .send_message::<InputChannel, InputMessage<A>>(&message)
        .unwrap_or_else(|err| {
            error!("Error while sending input message: {:?}", err);
        })
    // }

    // NOTE: actually we keep the input values! because they might be needed when we rollback for client prediction
    // TODO: figure out when we can delete old inputs. Basically when the oldest prediction group tick has passed?
    //  maybe at interpolation_tick(), since it's before any latest server update we receive?
}

fn receive_tick_events<A: LeafwingUserAction>(
    mut tick_events: EventReader<TickEvent>,
    mut global_input_buffer: Option<ResMut<InputBuffer<A>>>,
    mut input_buffer_query: Query<&mut InputBuffer<A>>,
) {
    for tick_event in tick_events.read() {
        match tick_event {
            TickEvent::TickSnap { old_tick, new_tick } => {
                if let Some(ref mut global_input_buffer) = global_input_buffer {
                    if let Some(start_tick) = global_input_buffer.start_tick {
                        trace!(
                            "Receive tick snap event {:?}. Updating global input buffer start_tick!",
                            tick_event
                        );
                        global_input_buffer.start_tick = Some(start_tick + (*new_tick - *old_tick));
                    }
                }
                for mut input_buffer in input_buffer_query.iter_mut() {
                    if let Some(start_tick) = input_buffer.start_tick {
                        input_buffer.start_tick = Some(start_tick + (*new_tick - *old_tick));
                        debug!(
                            "Receive tick snap event {:?}. Updating input buffer start_tick to {:?}!",
                            tick_event, input_buffer.start_tick
                        );
                    }
                }
            }
        }
    }
}

/// Read the InputMessages of other clients from the server to update the ActionDiffBuffers
/// This is useful if we want to do prediction for other clients.
///
/// The Predicted entity must have the ActionState component.
/// We will apply the diffs on the Predicted entity.
fn receive_remote_player_input_messages<A: LeafwingUserAction>(
    tick_manager: Res<TickManager>,
    mut connection: ResMut<ConnectionManager>,
    prediction_manager: Res<PredictionManager>,
    message_registry: Res<MessageRegistry>,
    // TODO: currently we do not handle entities that are controlled by multiple clients
    confirmed_query: Query<&Confirmed, Without<InputMap<A>>>,
    mut predicted_query: Query<&mut InputBuffer<A>, (Without<InputMap<A>>, With<Predicted>)>,
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
                    // TODO: fix this, very ugly
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
                            InputTarget::PrePredictedEntity(entity) => Some(entity),
                            InputTarget::Global => continue,
                        };
                        if let Some(entity) = entity {
                            debug!(
                                "received input message for entity: {:?}. Applying to diff buffer.",
                                entity
                            );
                            if let Ok(confirmed) = confirmed_query.get(*entity) {
                                if let Some(predicted) = confirmed.predicted {
                                    if let Ok(mut input_buffer) = predicted_query.get_mut(predicted)
                                    {
                                        debug!(?entity, ?diffs, end_tick = ?message.end_tick, "update action diff buffer for remote player PREDICTED using input message");
                                        input_buffer.update_from_message(
                                            message.end_tick,
                                            start,
                                            diffs,
                                        );
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
    use crate::tests::stepper::{BevyStepper, Step};

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
                input_delay_ticks: delay_ticks,
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
            .world
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
            .world
            .entity(server_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .is_some());

        // check that the entity is replicated, including the ActionState component
        let client_entity = *stepper
            .client_app
            .world
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();
        stepper
            .client_app
            .world
            .entity_mut(client_entity)
            .insert(InputMap::<LeafwingInput1>::new([(
                LeafwingInput1::Jump,
                KeyCode::KeyA,
            )]));
        assert!(stepper
            .client_app
            .world
            .entity(client_entity)
            .get::<ActionState<LeafwingInput1>>()
            .is_some());
        stepper.frame_step();
        (server_entity, client_entity)
    }

    /// Check that ActionStates are stored correctly in the InputBuffer
    #[test]
    fn test_buffer_inputs_no_delay() {
        let mut stepper = BevyStepper::default();
        let (server_entity, client_entity) = setup(&mut stepper);

        // press on a key
        stepper
            .client_app
            .world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyA);
        stepper.frame_step();
        let client_tick = stepper.client_tick();
        let input_buffer = stepper
            .client_app
            .world
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
            .world
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
            .world
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyA);
        stepper.frame_step();
        let input_buffer = stepper
            .client_app
            .world
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
    #[test]
    fn test_buffer_inputs_with_delay() {
        let mut stepper = build_stepper_with_input_delay(1);
        let (server_entity, client_entity) = setup(&mut stepper);

        // press on a key
        stepper
            .client_app
            .world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyA);
        stepper.frame_step();
        let client_tick = stepper.client_tick();
        // check that the action state got buffered without any press (because the input is delayed)
        // (we cannot use JustPressed because we start by ticking the ActionState)
        assert!(stepper
            .client_app
            .world
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap()
            .get(client_tick)
            .unwrap()
            .get_pressed()
            .is_empty());
        // after FixedUpdate runs, the ActionState should be stayed to the delayed action
        assert!(stepper
            .client_app
            .world
            .entity(client_entity)
            .get::<ActionState<LeafwingInput1>>()
            .unwrap()
            .pressed(&LeafwingInput1::Jump));

        // release the key
        stepper
            .client_app
            .world
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyA);
        // step another frame, this time we get the buffered input from earlier
        stepper.frame_step();
        let input_buffer = stepper
            .client_app
            .world
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
            .world
            .entity(client_entity)
            .get::<ActionState<LeafwingInput1>>()
            .unwrap()
            .get_pressed()
            .is_empty());

        stepper.frame_step();

        assert!(stepper
            .client_app
            .world
            .entity(client_entity)
            .get::<InputBuffer<LeafwingInput1>>()
            .unwrap()
            .get(client_tick + 2)
            .unwrap()
            .get_pressed()
            .is_empty());
    }
}
