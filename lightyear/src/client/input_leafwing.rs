//! Handles client-generated inputs
use bevy::prelude::*;
use leafwing_input_manager::plugin::InputManagerSystem;

use leafwing_input_manager::prelude::*;

use tracing::{error, info, trace};

use crate::channel::builder::InputChannel;
use crate::client::events::InputEvent;
use crate::client::prediction::plugin::is_in_rollback;
use crate::client::prediction::{Rollback, RollbackState};
use crate::client::resource::Client;
use crate::client::sync::client_is_synced;
use crate::inputs::leafwing::input_buffer::{InputBuffer, InputMessage};
use crate::inputs::leafwing::UserAction;
use crate::protocol::Protocol;
use crate::shared::sets::{FixedUpdateSet, MainSet};

#[derive(Debug, Clone)]
pub struct LeafwingInputConfig {
    /// How many consecutive packets lossed do we want to handle?
    /// This is used to compute the redundancy of the input messages.
    /// For instance, a value of 3 means that each input packet will contain the inputs for all the ticks
    ///  for the 3 last packets.
    packet_redundancy: u16,
}

impl Default for LeafwingInputConfig {
    fn default() -> Self {
        LeafwingInputConfig {
            packet_redundancy: 10,
        }
    }
}

pub struct LeafwingInputPlugin<P: Protocol, A: UserAction> {
    config: LeafwingInputConfig,
    _protocol_marker: std::marker::PhantomData<P>,
    _action_marker: std::marker::PhantomData<A>,
}

impl<P: Protocol, A: UserAction> LeafwingInputPlugin<P, A> {
    fn new(config: LeafwingInputConfig) -> Self {
        Self {
            config,
            _protocol_marker: std::marker::PhantomData,
            _action_marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol, A: UserAction> Default for LeafwingInputPlugin<P, A> {
    fn default() -> Self {
        Self {
            config: LeafwingInputConfig::default(),
            _protocol_marker: std::marker::PhantomData,
            _action_marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol, A: UserAction + TypePath> Plugin for LeafwingInputPlugin<P, A>
where
    P::Message: From<InputMessage<A>>,
{
    fn build(&self, app: &mut App) {
        // PLUGINS
        app.add_plugins(InputManagerPlugin::<A>::default());
        // RESOURCES
        // app.init_resource::<ActionState<A>>();
        app.init_resource::<InputBuffer<A>>();
        // SETS
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
            add_action_state_buffer::<A>.after(MainSet::ReceiveFlush),
        );
        app.add_systems(
            FixedUpdate,
            (
                buffer_action_state::<P, A>.run_if(not(is_in_rollback)),
                get_rollback_action_state::<A>.run_if(is_in_rollback),
            )
                .in_set(InputSystemSet::BufferInputs),
        );
        // in case the framerate is faster than fixed-update interval, we also write/clear the events at frame limits
        // TODO: should we also write the events at PreUpdate?
        // app.add_systems(PostUpdate, clear_input_events::<P>);
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

/// For each entity that has an action-state, insert an action-state-buffer
/// that will store the value of the action-state for the last few ticks
fn add_action_state_buffer<A: UserAction>(
    mut commands: Commands,
    action_state: Query<Entity, Added<ActionState<A>>>,
) {
    for entity in action_state.iter() {
        trace!("adding actions state buffer");
        commands.entity(entity).insert(InputBuffer::<A>::default());
    }
}

// non rollback: action-state have been written for us, nothing to do
// rollback: revert to the past action-state, then apply diffs?

// Write the value of the ActionStates for the current tick in the InputBuffer
// We do not need to buffer inputs during rollback, as they have already been buffered
fn buffer_action_state<P: Protocol, A: UserAction>(
    // TODO: get tick from tick_manager, not client
    client: ResMut<Client<P>>,
    mut global_input_buffer: ResMut<InputBuffer<A>>,
    global_action_state: Option<Res<ActionState<A>>>,
    mut action_state_query: Query<(Entity, &ActionState<A>, &mut InputBuffer<A>)>,
) {
    let tick = client.tick();
    for (entity, action_state, mut input_buffer) in action_state_query.iter_mut() {
        trace!("buffer action in input buffer: {:?}", action_state);
        input_buffer.set(tick, action_state);
        trace!("input buffer: {:?}", input_buffer);
    }
    if let Some(action_state) = global_action_state {
        global_input_buffer.set(tick, action_state.as_ref());
    }
}

// During rollback, fetch the action-state from the history for the corresponding tick and use that
// to set the ActionState resource/component
fn get_rollback_action_state<A: UserAction>(
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
        info!("get rollback action state");
        *action_state = input_buffer
            .get(tick)
            .unwrap_or(&ActionState::<A>::default())
            .clone();
    }
    if let Some(mut action_state) = global_action_state {
        *action_state = global_input_buffer.get(tick).unwrap().clone();
    }
}

// Take the input buffer, and prepare the input message to send to the server
fn prepare_input_message<P: Protocol, A: UserAction>(
    mut client: ResMut<Client<P>>,
    mut global_input_buffer: Option<ResMut<InputBuffer<A>>>,
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

    for (entity, mut input_buffer) in input_buffer_query.iter_mut() {
        trace!("adding input buffer to message");
        input_buffer.add_to_message(&mut message, current_tick, message_len, Some(entity));
        input_buffer.pop(interpolation_tick);
        info!("input buffer len: {:?}", input_buffer.buffer.len());
    }
    if let Some(mut input_buffer) = global_input_buffer {
        input_buffer.add_to_message(&mut message, current_tick, message_len, None);
        input_buffer.pop(interpolation_tick);
    }

    trace!("sending input message: {:?}", message);
    // all inputs are absent
    // TODO: should we provide variants of each user-facing function, so that it pushes the error
    //  to the ConnectionEvents?
    client
        .send_message::<InputChannel, InputMessage<A>>(message)
        .unwrap_or_else(|err| {
            error!("Error while sending input message: {:?}", err);
        })

    // NOTE: actually we keep the input values! because they might be needed when we rollback for client prediction
    // TODO: figure out when we can delete old inputs. Basically when the oldest prediction group tick has passed?
    //  maybe at interpolation_tick(), since it's before any latest server update we receive?
}
