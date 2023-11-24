use bevy::prelude::{
    EventReader, EventWriter, FixedUpdate, IntoSystemConfigs, IntoSystemSetConfigs, Plugin,
    PostUpdate, Res, ResMut, SystemSet,
};
use tracing::trace;

use crate::client::prediction::{Rollback, RollbackState};
use crate::client::Client;
use crate::plugin::events::InputEvent;
use crate::plugin::sets::{FixedUpdateSet, MainSet};
use crate::{App, InputChannel, Protocol, UserInput};

pub struct InputPlugin<P: Protocol> {
    _marker: std::marker::PhantomData<P>,
}

impl<P: Protocol> Default for InputPlugin<P> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData::default(),
        }
    }
}

/// Input of the user for the current tick
pub struct CurrentInput<T: UserInput> {
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
                InputSystemSet::BufferInputs.after(FixedUpdateSet::TickUpdate),
                InputSystemSet::WriteInputEvent
                    .before(FixedUpdateSet::Main)
                    .after(InputSystemSet::BufferInputs),
                InputSystemSet::ClearInputEvent.after(FixedUpdateSet::Main),
            ),
        );
        app.configure_sets(
            PostUpdate,
            InputSystemSet::PrepareInputMessage.before(MainSet::Send),
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
        app.add_systems(
            PostUpdate,
            prepare_input_message::<P>.in_set(InputSystemSet::PrepareInputMessage),
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
    PrepareInputMessage,
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
    // TODO: the number of messages should be in SharedConfig
    trace!(tick = ?client.tick(), "prepare_input_message");
    // TODO: instead of 15, send ticks up to the latest yet ACK-ed input tick
    //  this means we would also want to track packet->message acks for unreliable channels as well, so we can notify
    //  this system what the latest acked input tick is?
    let message = client.get_input_buffer().create_message(client.tick(), 15);
    client.buffer_send::<InputChannel, _>(message);
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
