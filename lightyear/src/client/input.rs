//! Handles client-generated inputs
use bevy::prelude::{
    not, App, EventReader, EventWriter, FixedUpdate, IntoSystemConfigs, IntoSystemSetConfigs,
    Plugin, PostUpdate, Res, ResMut, SystemSet,
};
use tracing::{debug, error, info, trace};

use crate::channel::builder::InputChannel;
use crate::client::config::ClientConfig;
use crate::client::connection::ConnectionManager;
use crate::client::events::InputEvent;
use crate::client::prediction::plugin::is_in_rollback;
use crate::client::prediction::{Rollback, RollbackState};
use crate::client::resource::Client;
use crate::client::sync::client_is_synced;
use crate::inputs::native::UserAction;
use crate::prelude::TickManager;
use crate::protocol::Protocol;
use crate::shared::sets::{FixedUpdateSet, MainSet};
use crate::shared::tick_manager::TickEvent;

#[derive(Debug, Clone)]
pub struct InputConfig {
    /// How many consecutive packets losses do we want to handle?
    /// This is used to compute the redundancy of the input messages.
    /// For instance, a value of 3 means that each input packet will contain the inputs for all the ticks
    ///  for the 3 last packets.
    pub(crate) packet_redundancy: u16,
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig {
            packet_redundancy: 10,
        }
    }
}

pub struct InputPlugin<P: Protocol> {
    config: InputConfig,
    _marker: std::marker::PhantomData<P>,
}

impl<P: Protocol> InputPlugin<P> {
    fn new(config: InputConfig) -> Self {
        Self {
            config,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol> Default for InputPlugin<P> {
    fn default() -> Self {
        Self {
            config: InputConfig::default(),
            _marker: std::marker::PhantomData,
        }
    }
}

/// Input of the user for the current tick
pub struct CurrentInput<T: UserAction> {
    // TODO: should we allow a Vec of inputs? for example if a user presses multiple buttons?
    //  or would that be encoded as a combination?
    input: T,
}

impl<P: Protocol> Plugin for InputPlugin<P> {
    fn build(&self, app: &mut App) {
        // EVENT
        app.add_event::<InputEvent<P::Input>>();
        // SETS
        app.configure_sets(
            FixedUpdate,
            (
                FixedUpdateSet::TickUpdate,
                // no need to keep buffering inputs during rollback
                InputSystemSet::BufferInputs.run_if(not(is_in_rollback)),
                InputSystemSet::WriteInputEvent,
                FixedUpdateSet::Main,
                InputSystemSet::ClearInputEvent,
            )
                .chain(),
        );
        app.configure_sets(
            PostUpdate,
            (
                // handle tick events from sync before sending the message
                InputSystemSet::ReceiveTickEvents
                    .after(MainSet::Sync)
                    .run_if(client_is_synced::<P>),
                // we send inputs only every send_interval
                InputSystemSet::SendInputMessage
                    .in_set(MainSet::Send)
                    .run_if(client_is_synced::<P>),
                MainSet::SendPackets,
            )
                .chain(),
        );

        // SYSTEMS
        app.add_systems(
            FixedUpdate,
            write_input_event::<P>.in_set(InputSystemSet::WriteInputEvent),
        );
        app.add_systems(
            FixedUpdate,
            clear_input_events::<P>.in_set(InputSystemSet::ClearInputEvent),
        );
        // in case the framerate is faster than fixed-update interval, we also write/clear the events at frame limits
        // TODO: should we also write the events at PreUpdate?
        // app.add_systems(PostUpdate, clear_input_events::<P>);
        app.add_systems(
            PostUpdate,
            (
                prepare_input_message::<P>.in_set(InputSystemSet::SendInputMessage),
                receive_tick_events::<P>.in_set(InputSystemSet::ReceiveTickEvents),
            ),
        );
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
    // FIXED UPDATE
    /// System Set to write the input events to the input buffer.
    /// The User should add their system here!!
    BufferInputs,
    /// FixedUpdate system to get any inputs from the client. This should be run before the game/physics logic
    WriteInputEvent,
    /// System Set to clear the input events (otherwise bevy clears events every frame, not every tick)
    ClearInputEvent,

    // POST UPDATE
    /// In case we suddenly changed the ticks during sync, we need to update out input buffers to the new ticks
    ReceiveTickEvents,
    /// System Set to prepare the input message (in Send SystemSet)
    SendInputMessage,
}

// /// Runs at the start of every FixedUpdate schedule
// /// The USER must write this. Check what inputs were pressed and add them to the input buffer
// /// DO NOT RUN THIS DURING ROLLBACK
// fn update_input_buffer<P: Protocol>(mut client: ResMut<Client<P>>) {}
//
// // system that runs after update_input_buffer, and uses the input to update the world?
// // - NOT rollback: gets the input for the current tick from the input buffer, only runs on predicted entities.
// // - IN rollback: gets the input for the current rollback tick from the input buffer, only runs on predicted entities.
// fn apply_input() {}

/// System that clears the input events.
/// It is necessary because events are cleared every frame, but we want to clear every tick instead
fn clear_input_events<P: Protocol>(mut input_events: EventReader<InputEvent<P::Input>>) {
    input_events.clear();
}

// Create a system that reads from the input buffer and returns the inputs of all clients for the current tick.
// The only tricky part is that events are cleared every frame, but we want to clear every tick instead
// Do it in this system because we want an input for every tick
fn write_input_event<P: Protocol>(
    tick_manager: Res<TickManager>,
    connection: Res<ConnectionManager<P>>,
    mut input_events: EventWriter<InputEvent<P::Input>>,
    rollback: Option<Res<Rollback>>,
) {
    let tick = rollback.map_or(tick_manager.tick(), |rollback| match rollback.state {
        RollbackState::Default => tick_manager.tick(),
        RollbackState::ShouldRollback {
            current_tick: rollback_tick,
        } => rollback_tick,
    });
    input_events.send(InputEvent::new(connection.get_input(tick), ()));
}

fn receive_tick_events<P: Protocol>(
    mut tick_events: EventReader<TickEvent>,
    mut connection: ResMut<ConnectionManager<P>>,
) {
    for tick_event in tick_events.read() {
        match tick_event {
            TickEvent::TickSnap { old_tick, new_tick } => {
                // if the tick got updated, update our inputs to match our new ticks
                if let Some(start_tick) = connection.input_buffer.start_tick {
                    trace!(
                        "Receive tick snap event {:?}. Updating input buffer start_tick!",
                        tick_event
                    );
                    connection.input_buffer.start_tick = Some(start_tick + (*new_tick - *old_tick));
                };
            }
        }
    }
}

// Take the input buffer, and prepare the input message to send to the server
fn prepare_input_message<P: Protocol>(
    mut connection: ResMut<ConnectionManager<P>>,
    config: Res<ClientConfig>,
    tick_manager: Res<TickManager>,
) {
    let current_tick = tick_manager.tick();
    // TODO: the number of messages should be in SharedConfig
    trace!(tick = ?current_tick, "prepare_input_message");
    // TODO: instead of 15, send ticks up to the latest yet ACK-ed input tick
    //  this means we would also want to track packet->message acks for unreliable channels as well, so we can notify
    //  this system what the latest acked input tick is?

    // we send redundant inputs, so that if a packet is lost, we can still recover
    let num_tick: u16 = ((config.shared.client_send_interval.as_nanos()
        / config.shared.tick.tick_duration.as_nanos())
        + 1)
    .try_into()
    .unwrap();
    let redundancy = config.input.packet_redundancy;
    // let redundancy = 3;
    let message_len = redundancy * num_tick;
    // TODO: we can either:
    //  - buffer an input message at every tick, and not require that much redundancy
    //  - buffer an input every frame; and require some redundancy (number of tick per frame)
    //  - or buffer an input only when we are sending, and require more redundancy
    // let message_len = 20 as u16;
    let message = connection
        .input_buffer
        .create_message(tick_manager.tick(), message_len);
    // all inputs are absent
    if !message.is_empty() {
        // TODO: should we provide variants of each user-facing function, so that it pushes the error
        //  to the ConnectionEvents?
        debug!("sending input message: {:?}", message.end_tick);
        connection
            .send_message::<InputChannel, _>(message)
            .unwrap_or_else(|err| {
                error!("Error while sending input message: {:?}", err);
            })
    }
    // NOTE: actually we keep the input values! because they might be needed when we rollback for client prediction
    // TODO: figure out when we can delete old inputs. Basically when the oldest prediction group tick has passed?
    //  maybe at interpolation_tick(), since it's before any latest server update we receive?

    // delete old input values
    let interpolation_tick = connection.sync_manager.interpolation_tick(&tick_manager);
    connection.input_buffer.pop(interpolation_tick);
    // .pop(current_tick - (message_len + 1));
}
