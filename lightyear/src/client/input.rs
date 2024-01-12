//! Handles client-generated inputs
use bevy::prelude::{
    not, App, EventReader, EventWriter, FixedUpdate, IntoSystemConfigs, IntoSystemSetConfigs,
    Plugin, PostUpdate, Res, ResMut, SystemSet,
};
use tracing::{error, trace};

use crate::channel::builder::InputChannel;
use crate::client::events::InputEvent;
use crate::client::prediction::plugin::is_in_rollback;
use crate::client::prediction::{Rollback, RollbackState};
use crate::client::resource::Client;
use crate::client::sync::client_is_synced;
use crate::inputs::native::UserAction;
use crate::protocol::Protocol;
use crate::shared::sets::{FixedUpdateSet, MainSet};
use crate::shared::tick_manager::TickManaged;

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
            ((
                FixedUpdateSet::TickUpdate,
                // no need to keep buffering inputs during rollback
                InputSystemSet::BufferInputs.run_if(not(is_in_rollback)),
                InputSystemSet::WriteInputEvent,
                FixedUpdateSet::Main,
                InputSystemSet::ClearInputEvent,
            )
                .chain(),),
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
            prepare_input_message::<P>
                .in_set(InputSystemSet::SendInputMessage)
                .run_if(client_is_synced::<P>),
        );
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
    /// System Set to write the input events to the input buffer.
    /// The User should add their system here!!
    BufferInputs,
    /// FixedUpdate system to get any inputs from the client. This should be run before the game/physics logic
    WriteInputEvent,
    /// System Set to clear the input events (otherwise bevy clears events every frame, not every tick)
    ClearInputEvent,
    /// System Set to prepare the input message
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
    mut client: ResMut<Client<P>>,
    mut input_events: EventWriter<InputEvent<P::Input>>,
    rollback: Option<Res<Rollback>>,
) {
    let tick = rollback.map_or(client.tick(), |rollback| match rollback.state {
        RollbackState::Default => client.tick(),
        RollbackState::ShouldRollback {
            current_tick: rollback_tick,
        } => rollback_tick,
    });
    input_events.send(InputEvent::new(client.get_input(tick).clone(), ()));
}

// Take the input buffer, and prepare the input message to send to the server
fn prepare_input_message<P: Protocol>(mut client: ResMut<Client<P>>) {
    let current_tick = client.tick();
    // TODO: the number of messages should be in SharedConfig
    trace!(tick = ?current_tick, "prepare_input_message");
    // TODO: instead of 15, send ticks up to the latest yet ACK-ed input tick
    //  this means we would also want to track packet->message acks for unreliable channels as well, so we can notify
    //  this system what the latest acked input tick is?

    // we send redundant inputs, so that if a packet is lost, we can still recover
    let num_tick = ((client.config().shared.client_send_interval.as_micros()
        / client.config().shared.tick.tick_duration.as_micros())
        + 1) as u16;
    let redundancy = client.config().input.packet_redundancy;
    // let redundancy = 3;
    let message_len = redundancy * num_tick;
    // TODO: we can either:
    //  - buffer an input message at every tick, and not require that much redundancy
    //  - buffer an input every frame; and require some redundancy (number of tick per frame)
    //  - or buffer an input only when we are sending, and require more redundancy
    // let message_len = 20 as u16;
    let message = client
        .get_input_buffer()
        .create_message(client.tick(), message_len);
    // all inputs are absent
    if !message.is_empty() {
        // TODO: should we provide variants of each user-facing function, so that it pushes the error
        //  to the ConnectionEvents?
        client
            .send_message::<InputChannel, _>(message)
            .unwrap_or_else(|err| {
                error!("Error while sending input message: {:?}", err);
            })
    }
    // NOTE: actually we keep the input values! because they might be needed when we rollback for client prediction
    // TODO: figure out when we can delete old inputs. Basically when the oldest prediction group tick has passed?
    //  maybe at interpolation_tick(), since it's before any latest server update we receive?

    // delete old input values
    let interpolation_tick = client
        .connection
        .sync_manager
        .interpolation_tick(&client.tick_manager);
    client.get_mut_input_buffer().pop(interpolation_tick);
    // .pop(current_tick - (message_len + 1));
}

// on the client:
// - FixedUpdate: before physics but after increment tick,
//   - rollback: we get the input from the history -> HERE GIVE THE USER AN OPPORTUNITY TO CUSTOMIZE.
//        BY DEFAULT WE JUST TAKE THE INPUT FOR THE TICK, BUT MAYBE WE WANT TO DO SOMETHING ELSE?
//        SLIGHTLY MODIFY THE INPUT? IF NONE, REPEAT THE PREVIOUS ONE?
//   - non rollback:
//         we get the input from keyboard/mouse and store it in the InputBuffer
//         use input for predicted entities
//   - can use system piping?
// - Send:
//   - we read the
