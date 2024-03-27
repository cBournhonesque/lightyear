//! Module to handle inputs that are defined using the `leafwing_input_manager` crate
//!
//! ## Creation
//!
//! You first need to create Inputs that are defined using the [`leafwing_input_manager`](https://github.com/Leafwing-Studios/leafwing-input-manager) crate.
//! (see the documentation of the crate for more information)
//! In particular your inputs should implement the [`Actionlike`] trait.
//! You will also need to implement the `LeafwingUserAction` trait
//!
//! ```no_run,ignore
//! # use lightyear::prelude::LeafwingUserAction;
//! #[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
//! pub enum PlayerActions {
//!     Up,
//!     Down,
//!     Left,
//!     Right,
//! }
//! impl LeafwingUserAction for PlayerActions {}
//! ```
//!
//! ## Usage
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
use bevy::utils::HashMap;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::*;
use tracing::{error, trace};

use crate::channel::builder::InputChannel;
use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::prediction::plugin::{is_in_rollback, PredictionSet};
use crate::client::prediction::{Predicted, Rollback, RollbackState};
use crate::client::sync::client_is_synced;
use crate::inputs::leafwing::input_buffer::{
    ActionDiff, ActionDiffBuffer, ActionDiffEvent, InputBuffer, InputMessage, InputTarget,
};
use crate::inputs::leafwing::LeafwingUserAction;
use crate::prelude::TickManager;
use crate::protocol::Protocol;
use crate::shared::replication::components::PrePredicted;
use crate::shared::sets::{FixedUpdateSet, MainSet};
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
#[derive(Debug, Clone, Resource)]
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

    /// If true, we only send diffs on the tick they were generated. (i.e. we will send a key-press only once)
    /// There is a risk that the packet arrives too late on the server and the server does not apply the diffs,
    /// which would break the input handling on the server.
    /// Turn this on if you want to optimize the bandwidth that the client sends to the server.
    pub send_diffs_only: bool,
    // TODO: add an option where we send all diffs vs send only just-pressed diffs
    pub _marker: std::marker::PhantomData<A>,
}

impl<A> Default for LeafwingInputConfig<A> {
    fn default() -> Self {
        LeafwingInputConfig {
            // input_delay_ticks: 0,
            packet_redundancy: 10,
            send_diffs_only: true,
            _marker: PhantomData,
        }
    }
}

impl<A> LeafwingInputConfig<A> {
    // pub fn with_input_delay_ticks(mut self, tick: u16) -> Self {
    //     self.input_delay_ticks = tick;
    //     self
    // }
}

/// Adds a plugin to handle inputs using the LeafwingInputManager
pub struct LeafwingInputPlugin<P: Protocol, A: LeafwingUserAction> {
    config: LeafwingInputConfig<A>,
    _protocol_marker: PhantomData<P>,
    _action_marker: PhantomData<A>,
}

impl<P: Protocol, A: LeafwingUserAction> LeafwingInputPlugin<P, A> {
    pub fn new(config: LeafwingInputConfig<A>) -> Self {
        Self {
            config,
            _protocol_marker: PhantomData,
            _action_marker: PhantomData,
        }
    }
}

impl<P: Protocol, A: LeafwingUserAction> Default for LeafwingInputPlugin<P, A> {
    fn default() -> Self {
        Self {
            config: LeafwingInputConfig::default(),
            _protocol_marker: PhantomData,
            _action_marker: PhantomData,
        }
    }
}

/// Returns true if there is input delay present
// fn is_input_delay<A: LeafwingUserAction>(config: Res<LeafwingInputConfig<A>>) -> bool {
//     config.input_delay_ticks > 0
// }

fn is_input_delay(config: Res<ClientConfig>) -> bool {
    config.prediction.input_delay_ticks > 0
}

impl<P: Protocol, A: LeafwingUserAction + TypePath> Plugin for LeafwingInputPlugin<P, A>
where
    P::Message: From<InputMessage<A>>,
    // FLOW WITH INPUT DELAY
    // - pre-update: run leafwing to update ActionState
    //   this is the action-state for tick T + delay

    // - fixed-update:
    //   - ONLY IF INPUT-DELAY IS NON ZERO. store the action-state in the buffer for tick T + delay
    //   - generate the action-diffs for tick T + delay (using the ActionState)
    //   - ONLY IF INPUT-DELAY IS NON ZERO. restore the action-state from the buffer for tick T
{
    fn build(&self, app: &mut App) {
        // PLUGINS
        app.add_plugins(InputManagerPlugin::<A>::default());
        // RESOURCES
        app.insert_resource(self.config.clone());
        app.init_resource::<ToggleActions<A>>();
        // app.init_resource::<ActionState<A>>();
        app.init_resource::<InputBuffer<A>>();
        app.init_resource::<ActionDiffBuffer<A>>();
        // app.init_resource::<LeafwingTickManager<A>>();
        app.init_resource::<Events<ActionDiffEvent<A>>>();
        // SETS
        // app.configure_sets(PreUpdate, InputManagerSystem::Tick.run_if(should_tick::<A>));
        app.configure_sets(FixedFirst, FixedUpdateSet::TickUpdate);
        app.configure_sets(FixedPreUpdate, InputSystemSet::BufferInputs);
        app.configure_sets(
            PostUpdate,
            // we send inputs only every send_interval
            (
                MainSet::Sync,
                // handle tick events from sync before sending the message
                // because sending the message might also modify the tick (when popping to interpolation tick)
                InputSystemSet::ReceiveTickEvents.run_if(client_is_synced::<P>),
                InputSystemSet::SendInputMessage
                    .run_if(client_is_synced::<P>)
                    .in_set(MainSet::Send),
                MainSet::SendPackets,
            )
                .chain(),
        );

        // SYSTEMS
        app.add_systems(
            PreUpdate,
            (
                generate_action_diffs::<A>
                    .run_if(run_if_enabled::<A>)
                    .after(InputManagerSystem::ReleaseOnDisable)
                    .after(InputManagerSystem::Update)
                    .after(InputManagerSystem::ManualControl)
                    .after(InputManagerSystem::Tick),
                add_action_state_buffer::<A>.after(PredictionSet::SpawnPredictionFlush),
            ),
        );
        // NOTE: we do not tick the ActionState during FixedUpdate
        // This means that an ActionState can stay 'JustPressed' for multiple ticks, if we have multiple tick within a single frame.
        // You have 2 options:
        // - handle `JustPressed` actions in the Update schedule, where they can only happen once
        // - `consume` the action when you read it, so that it can only happen once
        app.add_systems(
            FixedPreUpdate,
            (
                (
                    (write_action_diffs::<A>, buffer_action_state::<P, A>),
                    // get the action-state corresponding to the current tick (which we need to get from the buffer
                    //  because it was added to the buffer input_delay ticks ago)
                    get_non_rollback_action_state::<A>.run_if(is_input_delay),
                )
                    .chain()
                    .run_if(run_if_enabled::<A>.and_then(not(is_in_rollback))),
                get_rollback_action_state::<A>.run_if(run_if_enabled::<A>.and_then(is_in_rollback)),
            )
                .in_set(InputSystemSet::BufferInputs),
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
                    .and_then(not(is_in_rollback))
                    .and_then(run_if_enabled::<A>),
            ),
        );

        // in case the framerate is faster than fixed-update interval, we also write/clear the events at frame limits
        // TODO: should we also write the events at PreUpdate?
        // app.add_systems(PostUpdate, clear_input_events::<P>);

        // NOTE:
        // - maybe don't include the InputManagerPlugin for all ActionLike, but only for those that need to be replicated.
        //   For stuff that only affects the user, such as camera movement, there's no need to replicate the input?
        // - one thing to understand is that if we have F1 TA ( frame 1 starts, and then we run one FixedUpdate schedule)
        //   we want to add the input value computed during F1 to the buffer for tick TA, because the tick will use this value

        // NOTE: we run the buffer_action_state system in the Update for several reasons:
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
                receive_tick_events::<A>
                    .in_set(InputSystemSet::ReceiveTickEvents)
                    .run_if(run_if_enabled::<A>),
                prepare_input_message::<P, A>
                    .in_set(InputSystemSet::SendInputMessage)
                    .run_if(run_if_enabled::<A>),
                add_action_state_buffer_added_input_map::<A>,
                toggle_actions::<A>,
            ),
        );
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
    // FIXED UPDATE
    /// System Set where we update the InputBuffers
    /// - no rollback: we write the ActionState to the InputBuffers
    /// - rollback: we fetch the ActionState value from the InputBuffers
    BufferInputs,

    // POST UPDATE
    /// In case we suddenly changed the ticks during sync, we need to update out input buffers to the new ticks
    ReceiveTickEvents,
    /// System Set to prepare the input message
    SendInputMessage,
}

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
        commands.entity(entity).insert((
            InputBuffer::<A>::default(),
            ActionDiffBuffer::<A>::default(),
        ));
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
    // we only add the action state buffer to predicted entities (which are controlled by the user)
    predicted_entities: Query<
        Entity,
        (
            Added<ActionState<A>>,
            With<InputMap<A>>,
            Without<InputBuffer<A>>, // Or<(With<Predicted>, With<ShouldBePredicted>)>,
        ),
    >,
    // other_entities: Query<
    //     Entity,
    //     (
    //         Added<ActionState<A>>,
    //         Without<Predicted>,
    //         Without<ShouldBePredicted>,
    //     ),
    // >,
) {
    // TODO: find a way to add input-buffer/action-diff-buffer only for controlled entity
    //  maybe provide the "controlled" component? or just use With<InputMap>?

    for entity in predicted_entities.iter() {
        trace!(?entity, "adding actions state buffer");
        // TODO: THIS SHOULD ONLY BE FOR THE ENTITIES CONTROLLED BY THE CLIENT, SO MAYBE ADD THEM MANUALLY?
        //   BECAUSE WHEN PREDICTING OTHER PLAYERS, WE DO NOT WANT TO ADD THE ACTION STATE BUFFER
        commands.entity(entity).insert((
            InputBuffer::<A>::default(),
            ActionDiffBuffer::<A>::default(),
        ));
    }
    // for entity in other_entities.iter() {
    //     trace!(?entity, "REMOVING ACTION STATE FOR CONFIRMED");
    //     commands.entity(entity).remove::<ActionState<A>>();
    // }
}

/// At the start of the frame, restore the ActionState to the latest-action state in buffer
/// (e.g. the delayed action state) because all inputs are applied to the delayed action-state.
fn get_delayed_action_state<A: LeafwingUserAction>(
    global_input_buffer: Res<InputBuffer<A>>,
    global_action_state: Option<ResMut<ActionState<A>>>,
    mut action_state_query: Query<(Entity, &mut ActionState<A>, &InputBuffer<A>)>,
) {
    for (entity, mut action_state, input_buffer) in action_state_query.iter_mut() {
        // TODO: lots of clone + is complicated. Shouldn't we just have a DelayedActionState component + resource?
        //  the problem is that the Leafwing Plugin works on ActionState directly...
        *action_state = input_buffer
            .get_last()
            .unwrap_or(&ActionState::<A>::default())
            .clone();
        trace!("restored delayed action state");
    }
    if let Some(mut action_state) = global_action_state {
        *action_state = global_input_buffer.get_last().unwrap().clone();
    }
}

// non rollback: action-state have been written for us, nothing to do
// rollback: revert to the past action-state, then apply diffs?

/// Write the value of the ActionStates for the current tick in the InputBuffer
/// We do not need to buffer inputs during rollback, as they have already been buffered
fn buffer_action_state<P: Protocol, A: LeafwingUserAction>(
    config: Res<ClientConfig>,
    tick_manager: Res<TickManager>,
    mut global_input_buffer: ResMut<InputBuffer<A>>,
    global_action_state: Option<Res<ActionState<A>>>,
    mut action_state_query: Query<(Entity, &ActionState<A>, &mut InputBuffer<A>)>,
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
        trace!(
            ?entity,
            ?tick,
            "input buffer. Start tick {:?}, len: {:?}",
            input_buffer.start_tick,
            input_buffer.buffer.len()
        );
    }
    if let Some(action_state) = global_action_state {
        global_input_buffer.set(tick, action_state.as_ref());
    }
}

// TODO: combine this with the rollback function
// If we have input-delay, we need to set the ActionState for the current tick
// using the value stored in the buffer
fn get_non_rollback_action_state<A: LeafwingUserAction>(
    tick_manager: Res<TickManager>,
    global_input_buffer: Res<InputBuffer<A>>,
    global_action_state: Option<ResMut<ActionState<A>>>,
    mut action_state_query: Query<(Entity, &mut ActionState<A>, &InputBuffer<A>)>,
) {
    let tick = tick_manager.tick();
    for (entity, mut action_state, input_buffer) in action_state_query.iter_mut() {
        // let state_is_empty = input_buffer.get(tick).is_none();
        // let input_buffer = input_buffer.buffer;
        trace!(?entity, ?tick, "get action state. Buffer: {}", input_buffer);
        *action_state = input_buffer
            .get(tick)
            .unwrap_or(&ActionState::<A>::default())
            .clone();
        debug!(
            ?entity,
            ?tick,
            "fetched action state from buffer: {:?}",
            action_state.get_pressed()
        );
    }
    if let Some(mut action_state) = global_action_state {
        *action_state = global_input_buffer
            .get(tick)
            .unwrap_or(&ActionState::<A>::default())
            .clone();
    }
}

// During rollback, fetch the action-state from the history for the corresponding tick and use that
// to set the ActionState resource/component
// For actions from other players (with no InputBuffer), no need to do anything, because we just received their latest action
//  and we consider that they will keep playing that action in the future
// TODO: implement some decay for the rollback ActionState of other players?
fn get_rollback_action_state<A: LeafwingUserAction>(
    global_input_buffer: Res<InputBuffer<A>>,
    global_action_state: Option<ResMut<ActionState<A>>>,
    mut action_state_query: Query<(Entity, &mut ActionState<A>, &InputBuffer<A>)>,
    rollback: Res<Rollback>,
) {
    let tick = match rollback.state {
        RollbackState::Default => unreachable!(),
        RollbackState::ShouldRollback {
            current_tick: rollback_tick,
        } => rollback_tick,
    };
    for (entity, mut action_state, input_buffer) in action_state_query.iter_mut() {
        // let state_is_empty = input_buffer.get(tick).is_none();
        // let input_buffer = input_buffer.buffer;
        trace!(
            ?entity,
            ?tick,
            "get rollback action state. Buffer: {}",
            input_buffer
        );
        *action_state = input_buffer.get(tick).cloned().unwrap_or_default();
        trace!("updated action state for rollback: {:?}", action_state);
    }
    if let Some(mut action_state) = global_action_state {
        *action_state = global_input_buffer.get(tick).cloned().unwrap_or_default();
    }
}

/// Read the action-diffs and store them in a buffer.
/// NOTE: we have an ActionState buffer used for rollbacks,
/// and an ActionDiff buffer used for sending diffs to the server
/// maybe instead of an entire ActionState buffer, we can just store the oldest ActionState, and re-use the diffs
/// to compute the next ActionStates?
/// NOTE: since we're using diffs. we need to make sure that all our diffs are sent correctly to the server.
///  If a diff is missing, maybe the server should make a request and we send them the entire ActionState?
fn write_action_diffs<A: LeafwingUserAction>(
    config: Res<ClientConfig>,
    tick_manager: Res<TickManager>,
    mut global_action_diff_buffer: Option<ResMut<ActionDiffBuffer<A>>>,
    mut diff_buffer_query: Query<&mut ActionDiffBuffer<A>>,
    mut action_diff_event: ResMut<Events<ActionDiffEvent<A>>>,
) {
    let delay = config.prediction.input_delay_ticks as i16;
    let tick = tick_manager.tick() + delay;
    // we drain the events when reading them
    for event in action_diff_event.drain() {
        if let Some(entity) = event.owner {
            if let Ok(mut diff_buffer) = diff_buffer_query.get_mut(entity) {
                trace!(?entity, ?tick, ?delay, "write action diff");
                diff_buffer.set(tick, event.action_diff);
            }
        } else {
            if let Some(ref mut diff_buffer) = global_action_diff_buffer {
                trace!(?tick, ?delay, "write global action diff");
                diff_buffer.set(tick, event.action_diff);
            }
        }
    }
}

// Take the input buffer, and prepare the input message to send to the server
fn prepare_input_message<P: Protocol, A: LeafwingUserAction>(
    mut connection: ResMut<ConnectionManager<P>>,
    config: Res<ClientConfig>,
    tick_manager: Res<TickManager>,
    global_action_diff_buffer: Option<ResMut<ActionDiffBuffer<A>>>,
    global_input_buffer: Option<ResMut<InputBuffer<A>>>,
    mut action_diff_buffer_query: Query<(
        Entity,
        Option<&Predicted>,
        &mut ActionDiffBuffer<A>,
        Option<&PrePredicted>,
    )>,
    mut input_buffer_query: Query<(Entity, &mut InputBuffer<A>)>,
) where
    P::Message: From<InputMessage<A>>,
{
    let tick = tick_manager.tick() + config.prediction.input_delay_ticks as i16;
    // TODO: the number of messages should be in SharedConfig
    trace!(tick = ?tick, "prepare_input_message");
    // TODO: instead of redundancy, send ticks up to the latest yet ACK-ed input tick
    //  this means we would also want to track packet->message acks for unreliable channels as well, so we can notify
    //  this system what the latest acked input tick is?
    // we send redundant inputs, so that if a packet is lost, we can still recover
    // A redundancy of 2 means that we can recover from 1 lost packet
    let num_tick: u16 = ((config.shared.client_send_interval.as_nanos()
        / config.shared.tick.tick_duration.as_nanos())
        + 1)
    .try_into()
    .unwrap();
    let redundancy = config.input.packet_redundancy;
    let message_len = redundancy * num_tick;

    let mut message = InputMessage::<A>::new(tick);

    // delete old input values
    // anything beyond interpolation tick should be safe to be deleted
    let interpolation_tick = connection.sync_manager.interpolation_tick(&tick_manager);
    trace!(
        "popping all inputs since interpolation tick: {:?}",
        interpolation_tick
    );

    for (entity, predicted, mut action_diff_buffer, pre_predicted) in
        action_diff_buffer_query.iter_mut()
    {
        debug!(
            ?tick,
            ?interpolation_tick,
            ?entity,
            "Preparing input message with buffer: {:?}",
            action_diff_buffer.as_ref()
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
            // for which client the inputs were!

            // 0. the entity is pre-predicted
            action_diff_buffer.add_to_message(
                &mut message,
                tick,
                message_len,
                InputTarget::PrePredictedEntity(entity),
            );
        } else {
            // 1.if the entity is confirmed, we need to convert the entity to the server's entity
            // 2. the entity is predicted.
            // We need to first convert the entity to confirmed, and then from confirmed to remote
            if let Some(confirmed) = predicted.map_or(Some(entity), |p| p.confirmed_entity) {
                if let Some(server_entity) = connection
                    .replication_receiver
                    .remote_entity_map
                    .get_remote(confirmed)
                    .copied()
                {
                    debug!("sending input for server entity: {:?}. local entity: {:?}, confirmed: {:?}", server_entity, entity, confirmed);
                    action_diff_buffer.add_to_message(
                        &mut message,
                        tick,
                        message_len,
                        InputTarget::Entity(server_entity),
                    );
                }
            } else {
                debug!("not sending inputs because couldnt find server entity");
            }
        }

        action_diff_buffer.pop(interpolation_tick);
    }
    for (entity, mut input_buffer) in input_buffer_query.iter_mut() {
        trace!(
            ?tick,
            ?entity,
            "Preparing input message with buffer: {}",
            input_buffer.as_ref()
        );
        input_buffer.pop(interpolation_tick);
        trace!("input buffer len: {:?}", input_buffer.buffer.len());
    }
    if let Some(mut action_diff_buffer) = global_action_diff_buffer {
        action_diff_buffer.add_to_message(&mut message, tick, message_len, InputTarget::Global);
        action_diff_buffer.pop(interpolation_tick);
    }
    if let Some(mut input_buffer) = global_input_buffer {
        input_buffer.pop(interpolation_tick);
    }

    // all inputs are absent
    // TODO: should we provide variants of each user-facing function, so that it pushes the error
    //  to the ConnectionEvents?
    if !message.is_empty() {
        debug!(
            action = ?A::short_type_path(),
            ?tick,
            "sending input message: {:?}",
            message.diffs
        );
        connection
            .send_message::<InputChannel, InputMessage<A>>(message)
            .unwrap_or_else(|err| {
                error!("Error while sending input message: {:?}", err);
            })
    }

    // NOTE: actually we keep the input values! because they might be needed when we rollback for client prediction
    // TODO: figure out when we can delete old inputs. Basically when the oldest prediction group tick has passed?
    //  maybe at interpolation_tick(), since it's before any latest server update we receive?
}

fn receive_tick_events<A: LeafwingUserAction>(
    mut tick_events: EventReader<TickEvent>,
    mut global_action_diff_buffer: Option<ResMut<ActionDiffBuffer<A>>>,
    mut global_input_buffer: Option<ResMut<InputBuffer<A>>>,
    mut action_diff_buffer_query: Query<&mut ActionDiffBuffer<A>>,
    mut input_buffer_query: Query<&mut InputBuffer<A>>,
) {
    for tick_event in tick_events.read() {
        match tick_event {
            TickEvent::TickSnap { old_tick, new_tick } => {
                if let Some(ref mut action_diff_buffer) = global_action_diff_buffer {
                    if let Some(start_tick) = action_diff_buffer.start_tick {
                        trace!(
                            "Receive tick snap event {:?}. Updating global action diff buffer start_tick!",
                            tick_event
                        );
                        action_diff_buffer.start_tick = Some(start_tick + (*new_tick - *old_tick));
                    }
                }
                if let Some(ref mut global_input_buffer) = global_input_buffer {
                    if let Some(start_tick) = global_input_buffer.start_tick {
                        trace!(
                            "Receive tick snap event {:?}. Updating global input buffer start_tick!",
                            tick_event
                        );
                        global_input_buffer.start_tick = Some(start_tick + (*new_tick - *old_tick));
                    }
                }
                for mut action_diff_buffer in action_diff_buffer_query.iter_mut() {
                    if let Some(start_tick) = action_diff_buffer.start_tick {
                        action_diff_buffer.start_tick = Some(start_tick + (*new_tick - *old_tick));
                        debug!(
                            "Receive tick snap event {:?}. Updating action diff buffer start_tick to {:?}!",
                            tick_event, action_diff_buffer.start_tick
                        );
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

// TODO: should run this only for entities with InputMap?
/// Generates an [`Events`] stream of [`ActionDiff`] from [`ActionState`]
///
/// We run this in the PreUpdate stage so that we generate diffs even if the frame has no fixed-update schedule
pub fn generate_action_diffs<A: LeafwingUserAction>(
    config: Res<LeafwingInputConfig<A>>,
    action_state: Option<ResMut<ActionState<A>>>,
    action_state_query: Query<(Entity, &ActionState<A>)>,
    mut action_diffs: EventWriter<ActionDiffEvent<A>>,
    mut previous_values: Local<HashMap<A, HashMap<Option<Entity>, f32>>>,
    mut previous_axis_pairs: Local<HashMap<A, HashMap<Option<Entity>, Vec2>>>,
) {
    // we use None to represent the global ActionState
    let action_state_iter = action_state_query
        .iter()
        .map(|(entity, action_state)| (Some(entity), action_state))
        .chain(
            action_state
                .as_ref()
                .map(|action_state| (None, action_state.as_ref())),
        );
    for (maybe_entity, action_state) in action_state_iter {
        let mut diffs = vec![];
        // TODO: optimize config.send_diffs_only at compile time?
        if config.send_diffs_only {
            for action in action_state.get_just_pressed() {
                trace!(?action, consumed=?action_state.consumed(&action), "action is JustPressed!");
                let Some(action_data) = action_state.action_data(&action) else {
                    warn!("Action in ActionDiff has no data: was it generated correctly?");
                    continue;
                };
                match action_data.axis_pair {
                    Some(axis_pair) => {
                        diffs.push(ActionDiff::AxisPairChanged {
                            action: action.clone(),
                            axis_pair: axis_pair.into(),
                        });
                        previous_axis_pairs
                            .entry(action)
                            .or_default()
                            .insert(maybe_entity, axis_pair.xy());
                    }
                    None => {
                        let value = action_data.value;
                        diffs.push(if value == 1. {
                            ActionDiff::Pressed {
                                action: action.clone(),
                            }
                        } else {
                            ActionDiff::ValueChanged {
                                action: action.clone(),
                                value,
                            }
                        });
                        previous_values
                            .entry(action)
                            .or_default()
                            .insert(maybe_entity, value);
                    }
                }
            }
        }
        for action in action_state.get_pressed() {
            if config.send_diffs_only {
                // we already handled these cases above
                if action_state.just_pressed(&action) {
                    continue;
                }
            }
            trace!(?action, consumed=?action_state.consumed(&action), "action is pressed!");
            let Some(action_data) = action_state.action_data(&action) else {
                warn!("Action in ActionState has no data: was it generated correctly?");
                continue;
            };
            match action_data.axis_pair {
                Some(axis_pair) => {
                    if config.send_diffs_only {
                        let previous_axis_pairs =
                            previous_axis_pairs.entry(action.clone()).or_default();

                        if let Some(previous_axis_pair) = previous_axis_pairs.get(&maybe_entity) {
                            if *previous_axis_pair == axis_pair.xy() {
                                continue;
                            }
                        }
                        previous_axis_pairs.insert(maybe_entity, axis_pair.xy());
                    }
                    diffs.push(ActionDiff::AxisPairChanged {
                        action: action.clone(),
                        axis_pair: axis_pair.into(),
                    });
                }
                None => {
                    let value = action_data.value;
                    if config.send_diffs_only {
                        let previous_values = previous_values.entry(action.clone()).or_default();

                        if let Some(previous_value) = previous_values.get(&maybe_entity) {
                            if *previous_value == value {
                                trace!(?action, "Same value as last time; not sending diff");
                                continue;
                            }
                        }
                        previous_values.insert(maybe_entity, value);
                    }
                    diffs.push(if value == 1. && !config.send_diffs_only {
                        ActionDiff::Pressed {
                            action: action.clone(),
                        }
                    } else {
                        ActionDiff::ValueChanged {
                            action: action.clone(),
                            value,
                        }
                    });
                }
            }
        }
        for action in action_state
            .get_released()
            .iter()
            // If we only send diffs, just keep the JustReleased keys.
            // Consumed keys are marked as 'Release' so we need to handle them separately
            // (see https://github.com/Leafwing-Studios/leafwing-input-manager/issues/443)
            .filter(|action| {
                !config.send_diffs_only
                    || action_state.just_released(*action)
                    || action_state.consumed(*action)
            })
        {
            let just_released = action_state.just_released(action);
            let consumed = action_state.consumed(action);
            trace!(
                send_diffs=?config.send_diffs_only,
                ?just_released,
                ?consumed,
                "action released: {:?}", action
            );
            diffs.push(ActionDiff::Released {
                action: action.clone(),
            });
            if config.send_diffs_only {
                if let Some(previous_axes) = previous_axis_pairs.get_mut(action) {
                    previous_axes.remove(&maybe_entity);
                }
                if let Some(previous_values) = previous_values.get_mut(action) {
                    previous_values.remove(&maybe_entity);
                }
            }
        }

        if !diffs.is_empty() {
            trace!(send_diffs_only = ?config.send_diffs_only, ?maybe_entity, "writing action diffs: {:?}", diffs);
            action_diffs.send(ActionDiffEvent {
                owner: maybe_entity,
                action_diff: diffs,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::asset::AsyncReadExt;
    use bevy::input::InputPlugin;
    use bevy::prelude::*;
    use bevy::utils::Duration;
    use leafwing_input_manager::action_state::ActionState;
    use leafwing_input_manager::input_map::InputMap;

    use crate::client::sync::SyncConfig;
    use crate::inputs::leafwing::input_buffer::{ActionDiff, ActionDiffBuffer, ActionDiffEvent};
    use crate::prelude::client::{InterpolationConfig, PredictionConfig};
    use crate::prelude::server::LeafwingInputPlugin;
    use crate::prelude::{LinkConditionerConfig, SharedConfig, TickConfig};
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};

    use super::*;

    fn setup() -> (BevyStepper, Entity, Entity) {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(0),
            incoming_jitter: Duration::from_millis(0),
            incoming_loss: 0.0,
        };
        let sync_config = SyncConfig::default().speedup_factor(1.0);
        let prediction_config = PredictionConfig::default().disable(false);
        let interpolation_config = InterpolationConfig::default();
        let mut stepper = BevyStepper::new(
            shared_config,
            sync_config,
            prediction_config,
            interpolation_config,
            link_conditioner,
            frame_duration,
        );
        stepper
            .client_app
            .add_plugins((crate::client::input_leafwing::LeafwingInputPlugin::<
                MyProtocol,
                LeafwingInput1,
            >::default(), InputPlugin));
        // let press_action_id = stepper.client_app.world.register_system(press_action);
        stepper.server_app.add_plugins((
            LeafwingInputPlugin::<MyProtocol, LeafwingInput1>::default(),
            InputPlugin,
        ));
        stepper.init();

        // create an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn((
                InputMap::<LeafwingInput1>::new([(LeafwingInput1::Jump, KeyCode::KeyA)]),
                ActionState::<LeafwingInput1>::default(),
                Replicate::default(),
            ))
            .id();
        // we need to step twice because we run client before server
        stepper.frame_step();
        stepper.frame_step();

        // check that the server entity got a ActionDiffBuffer added to it
        assert!(stepper
            .server_app
            .world
            .entity(server_entity)
            .get::<ActionDiffBuffer<LeafwingInput1>>()
            .is_some());

        // check that the entity is replicated, including the ActionState component
        let client_entity = *stepper
            .client_app
            .world
            .resource::<ClientConnectionManager>()
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
            .is_some(),);
        stepper.frame_step();
        (stepper, server_entity, client_entity)
    }

    #[test]
    fn test_generate_action_diffs() {
        let (mut stepper, server_entity, client_entity) = setup();

        // press the jump button on the client
        stepper
            .client_app
            .world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyA);
        stepper.frame_step();

        // listen to the ActionDiff event
        let action_diff_events = stepper
            .client_app
            .world
            .get_resource_mut::<Events<ActionDiffEvent<LeafwingInput1>>>()
            .unwrap();
        for event in action_diff_events.get_reader().read(&action_diff_events) {
            assert_eq!(
                event.action_diff,
                vec![ActionDiff::Pressed {
                    action: LeafwingInput1::Jump,
                }]
            );
            assert_eq!(event.owner, Some(client_entity));
        }
    }
}
