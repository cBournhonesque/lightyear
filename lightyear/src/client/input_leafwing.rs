//! Handles client-generated inputs
use bevy::prelude::*;
use bevy::utils::HashMap;
use leafwing_input_manager::plugin::InputManagerSystem;

use leafwing_input_manager::prelude::*;

use crate::_reexport::ShouldBePredicted;
use tracing::{error, info, trace};

use crate::channel::builder::InputChannel;
use crate::client::components::Confirmed;
use crate::client::events::InputEvent;
use crate::client::prediction::plugin::{is_in_rollback, PredictionSet};
use crate::client::prediction::{Predicted, Rollback, RollbackState};
use crate::client::resource::Client;
use crate::client::sync::client_is_synced;
use crate::inputs::leafwing::input_buffer::{
    ActionDiff, ActionDiffBuffer, ActionDiffEvent, InputBuffer, InputMessage,
};
use crate::inputs::leafwing::LeafwingUserAction;
use crate::protocol::Protocol;
use crate::shared::sets::{FixedUpdateSet, MainSet};

#[derive(Debug, Clone)]
pub struct LeafwingInputConfig {
    /// How many consecutive packets losses do we want to handle?
    /// This is used to compute the redundancy of the input messages.
    /// For instance, a value of 3 means that each input packet will contain the inputs for all the ticks
    ///  for the 3 last packets.
    packet_redundancy: u16,
}

// #[derive(Resource, Default)]
// /// Check if we should tick the leafwing input manager
// /// In the situation F1 TA F2 F3 TB, we would like to not tick at the beginning of F3, so that we can send the diffs
// /// from both F2 and F3 to the server.
// /// NOTE: if this is too complicated, just replicate the entire ActionState instead of using diffs
// pub(crate) struct LeafwingTickManager<A: UserAction> {
//     // if this is true, we do not tick the leafwing input manager
//     should_not_tick: bool,
// }
//
// pub(crate) fn should_tick<A: UserAction>(manager: Res<LeafwingTickManager<A>>) -> bool {
//     !manager.should_not_tick
// }

impl Default for LeafwingInputConfig {
    fn default() -> Self {
        LeafwingInputConfig {
            packet_redundancy: 10,
        }
    }
}

pub struct LeafwingInputPlugin<P: Protocol, A: LeafwingUserAction> {
    config: LeafwingInputConfig,
    _protocol_marker: std::marker::PhantomData<P>,
    _action_marker: std::marker::PhantomData<A>,
}

impl<P: Protocol, A: LeafwingUserAction> LeafwingInputPlugin<P, A> {
    fn new(config: LeafwingInputConfig) -> Self {
        Self {
            config,
            _protocol_marker: std::marker::PhantomData,
            _action_marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol, A: LeafwingUserAction> Default for LeafwingInputPlugin<P, A> {
    fn default() -> Self {
        Self {
            config: LeafwingInputConfig::default(),
            _protocol_marker: std::marker::PhantomData,
            _action_marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol, A: LeafwingUserAction + TypePath> Plugin for LeafwingInputPlugin<P, A>
where
    P::Message: From<InputMessage<A>>,
{
    fn build(&self, app: &mut App) {
        // PLUGINS
        app.add_plugins(InputManagerPlugin::<A>::default());
        // RESOURCES
        // app.init_resource::<ActionState<A>>();
        app.init_resource::<InputBuffer<A>>();
        app.init_resource::<ActionDiffBuffer<A>>();
        // app.init_resource::<LeafwingTickManager<A>>();
        app.init_resource::<Events<ActionDiffEvent<A>>>();
        // SETS
        // app.configure_sets(PreUpdate, InputManagerSystem::Tick.run_if(should_tick::<A>));
        app.configure_sets(
            FixedUpdate,
            (
                FixedUpdateSet::TickUpdate,
                InputSystemSet::BufferInputs,
                FixedUpdateSet::Main,
            )
                .chain(),
        );
        app.configure_sets(
            PostUpdate,
            // we send inputs only every send_interval
            (
                InputSystemSet::SendInputMessage.in_set(MainSet::Send),
                MainSet::SendPackets,
            )
                .chain(),
        );

        // SYSTEMS
        app.add_systems(
            PreUpdate,
            (
                generate_action_diffs::<A>.after(InputManagerSystem::ReleaseOnDisable),
                add_action_state_buffer::<A>.after(PredictionSet::SpawnPredictionFlush),
                // // disable tick after the tick system, so that we can send the diffs correctly
                // // even if we do not have any FixedUpdate schedule run this frame
                // disable_tick::<A>.after(InputManagerSystem::Tick),
            ),
        );
        app.add_systems(
            FixedUpdate,
            (
                // enable_tick::<A>.run_if(not(is_in_rollback)),
                (write_action_diffs::<P, A>, buffer_action_state::<P, A>)
                    .run_if(not(is_in_rollback)),
                get_rollback_action_state::<A>.run_if(is_in_rollback),
            )
                .in_set(InputSystemSet::BufferInputs),
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
            prepare_input_message::<P, A>
                .in_set(InputSystemSet::SendInputMessage)
                .run_if(client_is_synced::<P>),
        );
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
    /// System Set where we update the InputBuffers
    /// - no rollback: we write the ActionState to the InputBuffers
    /// - rollback: we fetch the ActionState value from the InputBuffers
    BufferInputs,
    /// System Set to prepare the input message
    SendInputMessage,
}

// // TODO: make this behaviour optional?
// //   it might be useful to keep an action-state on confirmed entities?
//
// /// If we ran a FixedUpdate schedule this frame, we enable ticking for next frame
// fn enable_tick<A: UserAction>(manager: ResMut<LeafwingTickManager<A>>) {
//     manager.should_not_tick = false;
// }
//
// fn disable_tick<A: UserAction>(manager: ResMut<LeafwingTickManager<A>>) {
//     manager.should_not_tick = true;
// }

/// For each entity that has an action-state, insert an action-state-buffer
/// that will store the value of the action-state for the last few ticks
fn add_action_state_buffer<A: LeafwingUserAction>(
    mut commands: Commands,
    // we only add the action state buffer to predicted entities (which are controlled by the user)
    predicted_entities: Query<
        Entity,
        (
            Added<ActionState<A>>,
            Or<(With<Predicted>, With<ShouldBePredicted>)>,
        ),
    >,
    other_entities: Query<
        Entity,
        (
            Added<ActionState<A>>,
            Without<Predicted>,
            Without<ShouldBePredicted>,
        ),
    >,
) {
    for entity in predicted_entities.iter() {
        trace!(?entity, "adding actions state buffer");
        commands.entity(entity).insert((
            InputBuffer::<A>::default(),
            ActionDiffBuffer::<A>::default(),
        ));
    }
    for entity in other_entities.iter() {
        trace!(?entity, "REMOVING ACTION STATE FOR CONFIRMED");
        commands.entity(entity).remove::<ActionState<A>>();
    }
}

// non rollback: action-state have been written for us, nothing to do
// rollback: revert to the past action-state, then apply diffs?

// Write the value of the ActionStates for the current tick in the InputBuffer
// We do not need to buffer inputs during rollback, as they have already been buffered
fn buffer_action_state<P: Protocol, A: LeafwingUserAction>(
    // TODO: get tick from tick_manager, not client
    client: ResMut<Client<P>>,
    mut global_input_buffer: ResMut<InputBuffer<A>>,
    global_action_state: Option<Res<ActionState<A>>>,
    mut action_state_query: Query<(Entity, &ActionState<A>, &mut InputBuffer<A>)>,
) {
    let tick = client.tick();
    for (entity, action_state, mut input_buffer) in action_state_query.iter_mut() {
        trace!(
            ?entity,
            ?tick,
            "ACTION_STATE: JUST PRESSED: {:?}/ JUST RELEASED: {:?}/ PRESSED: {:?}/ RELEASED: {:?}",
            action_state.get_just_pressed(),
            action_state.get_just_released(),
            action_state.get_pressed(),
            action_state.get_released(),
        );
        trace!(?entity, ?tick, "set action state in input buffer");
        input_buffer.set(tick, action_state);
        trace!("input buffer: {:?}", input_buffer);
    }
    if let Some(action_state) = global_action_state {
        global_input_buffer.set(tick, action_state.as_ref());
    }
}

// During rollback, fetch the action-state from the history for the corresponding tick and use that
// to set the ActionState resource/component
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
        *action_state = input_buffer
            .get(tick)
            .unwrap_or(&ActionState::<A>::default())
            .clone();
        trace!("updated action state for rollback: {:?}", action_state);
    }
    if let Some(mut action_state) = global_action_state {
        *action_state = global_input_buffer.get(tick).unwrap().clone();
    }
}

/// Read the action-diffs and store them in a buffer.
/// NOTE: we have an ActionState buffer used for rollbacks,
/// and an ActionDiff buffer used for sending diffs to the server
/// maybe instead of an entire ActionState buffer, we can just store the oldest ActionState, and re-use the diffs
/// to compute the next ActionStates?
/// NOTE: since we're using diffs. we need to make sure that all our diffs are sent correctly to the server.
///  If a diff is missing, maybe the server should make a request and we send them the entire ActionState?
fn write_action_diffs<P: Protocol, A: LeafwingUserAction>(
    client: Res<Client<P>>,
    mut global_action_diff_buffer: Option<ResMut<ActionDiffBuffer<A>>>,
    mut diff_buffer_query: Query<&mut ActionDiffBuffer<A>>,
    mut action_diff_event: ResMut<Events<ActionDiffEvent<A>>>,
) {
    let tick = client.tick();
    // we drain the events when reading them
    for event in action_diff_event.drain() {
        if let Some(entity) = event.owner {
            if let Ok(mut diff_buffer) = diff_buffer_query.get_mut(entity) {
                trace!(?entity, ?tick, "write action diff");
                diff_buffer.set(tick, event.action_diff);
            }
        } else {
            if let Some(ref mut diff_buffer) = global_action_diff_buffer {
                trace!(?tick, "write global action diff");
                diff_buffer.set(tick, event.action_diff);
            }
        }
    }
}

// Take the input buffer, and prepare the input message to send to the server
fn prepare_input_message<P: Protocol, A: LeafwingUserAction>(
    mut client: ResMut<Client<P>>,
    global_action_diff_buffer: Option<ResMut<ActionDiffBuffer<A>>>,
    global_input_buffer: Option<ResMut<InputBuffer<A>>>,
    mut action_diff_buffer_query: Query<(Entity, &mut ActionDiffBuffer<A>)>,
    mut input_buffer_query: Query<(Entity, &mut InputBuffer<A>)>,
) where
    P::Message: From<InputMessage<A>>,
{
    let current_tick = client.tick();
    // TODO: the number of messages should be in SharedConfig
    trace!(tick = ?current_tick, "prepare_input_message");
    // TODO: instead of redundancy, send ticks up to the latest yet ACK-ed input tick
    //  this means we would also want to track packet->message acks for unreliable channels as well, so we can notify
    //  this system what the latest acked input tick is?
    // we send redundant inputs, so that if a packet is lost, we can still recover
    // A redundancy of 2 means that we can recover from 1 lost packet
    let num_tick = ((client.config().shared.client_send_interval.as_micros()
        / client.config().shared.tick.tick_duration.as_micros())
        + 1) as u16;
    let redundancy = client.config().input.packet_redundancy;
    let message_len = redundancy * num_tick;

    let mut message = InputMessage::<A>::new(current_tick);

    // delete old input values
    // anything beyond interpolation tick should be safe to be deleted
    let interpolation_tick = client
        .connection
        .sync_manager
        .interpolation_tick(&client.tick_manager);

    for (entity, mut action_diff_buffer) in action_diff_buffer_query.iter_mut() {
        trace!(
            ?current_tick,
            ?entity,
            "Preparing input message with buffer: {:?}",
            action_diff_buffer.as_ref()
        );
        action_diff_buffer.add_to_message(&mut message, current_tick, message_len, Some(entity));
        action_diff_buffer.pop(interpolation_tick);
    }
    for (entity, mut input_buffer) in input_buffer_query.iter_mut() {
        trace!(
            ?current_tick,
            ?entity,
            "Preparing input message with buffer: {}",
            input_buffer.as_ref()
        );
        input_buffer.pop(interpolation_tick);
        trace!("input buffer len: {:?}", input_buffer.buffer.len());
    }
    if let Some(mut action_diff_buffer) = global_action_diff_buffer {
        action_diff_buffer.add_to_message(&mut message, current_tick, message_len, None);
        action_diff_buffer.pop(interpolation_tick);
    }
    if let Some(mut input_buffer) = global_input_buffer {
        input_buffer.pop(interpolation_tick);
    }

    // all inputs are absent
    // TODO: should we provide variants of each user-facing function, so that it pushes the error
    //  to the ConnectionEvents?
    if !message.is_empty() {
        trace!("sending input message: {:?}", message);
        client
            .send_message::<InputChannel, InputMessage<A>>(message)
            .unwrap_or_else(|err| {
                error!("Error while sending input message: {:?}", err);
            })
    }

    // NOTE: actually we keep the input values! because they might be needed when we rollback for client prediction
    // TODO: figure out when we can delete old inputs. Basically when the oldest prediction group tick has passed?
    //  maybe at interpolation_tick(), since it's before any latest server update we receive?
}

/// Generates an [`Events`] stream of [`ActionDiff`] from [`ActionState`]
///
/// This system is not part of the [`InputManagerPlugin`](crate::plugin::InputManagerPlugin) and must be added manually.
// TODO: to keep correctness even in case of an input packet arriving too late on the server,
//  we could generate a Diff even if the action is the same as the previous tick!
pub fn generate_action_diffs<A: Actionlike>(
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
        for action in action_state.get_just_pressed() {
            match action_state.action_data(action.clone()).axis_pair {
                Some(axis_pair) => {
                    diffs.push(ActionDiff::AxisPairChanged {
                        action: action.clone(),
                        axis_pair: axis_pair.into(),
                    });
                    previous_axis_pairs
                        .raw_entry_mut()
                        .from_key(&action)
                        .or_insert_with(|| (action.clone(), HashMap::default()))
                        .1
                        .insert(maybe_entity, axis_pair.xy());
                }
                None => {
                    let value = action_state.value(action.clone());
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
                        .raw_entry_mut()
                        .from_key(&action)
                        .or_insert_with(|| (action.clone(), HashMap::default()))
                        .1
                        .insert(maybe_entity, value);
                }
            }
        }
        for action in action_state.get_pressed() {
            if action_state.just_pressed(action.clone()) {
                continue;
            }
            match action_state.action_data(action.clone()).axis_pair {
                Some(axis_pair) => {
                    let previous_axis_pairs = previous_axis_pairs.get_mut(&action).unwrap();

                    if let Some(previous_axis_pair) = previous_axis_pairs.get(&maybe_entity) {
                        if *previous_axis_pair == axis_pair.xy() {
                            continue;
                        }
                    }
                    diffs.push(ActionDiff::AxisPairChanged {
                        action: action.clone(),
                        axis_pair: axis_pair.into(),
                    });
                    previous_axis_pairs.insert(maybe_entity, axis_pair.xy());
                }
                None => {
                    let value = action_state.value(action.clone());
                    let previous_values = previous_values.get_mut(&action).unwrap();

                    if let Some(previous_value) = previous_values.get(&maybe_entity) {
                        if *previous_value == value {
                            continue;
                        }
                    }
                    diffs.push(ActionDiff::ValueChanged {
                        action: action.clone(),
                        value,
                    });
                    previous_values.insert(maybe_entity, value);
                }
            }
        }
        for action in action_state.get_just_released() {
            diffs.push(ActionDiff::Released {
                action: action.clone(),
            });
            if let Some(previous_axes) = previous_axis_pairs.get_mut(&action) {
                previous_axes.remove(&maybe_entity);
            }
            if let Some(previous_values) = previous_values.get_mut(&action) {
                previous_values.remove(&maybe_entity);
            }
        }
        if !diffs.is_empty() {
            trace!("WRITING ACTION DIFF EVENT");
            action_diffs.send(ActionDiffEvent {
                owner: maybe_entity,
                action_diff: diffs,
            });
        }
    }
}
