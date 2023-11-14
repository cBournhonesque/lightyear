use crate::client::Client;
use crate::plugin::sets::MainSet;
use crate::{App, InputChannel, PingChannel, Protocol, UserInput};
use bevy::prelude::{
    FixedUpdate, IntoSystemConfigs, IntoSystemSetConfigs, Plugin, PostUpdate, ResMut, SystemSet,
};
use tracing::{debug, trace};

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
        // insert the input buffer resource
        // SETS
        app.configure_sets(
            FixedUpdate,
            InputSystemSet::BufferInputs.before(MainSet::FixedUpdateGame),
        );
        app.configure_sets(
            PostUpdate,
            InputSystemSet::PrepareInputMessage.before(MainSet::Send),
        );

        // SYSTEMS
        app.add_systems(
            PostUpdate,
            prepare_input_message::<P>.in_set(InputSystemSet::PrepareInputMessage),
        );
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystemSet {
    /// FixedUpdate system to get any inputs from the client. This should be run before the game/physics logic
    BufferInputs,
    /// System Set to prepare the input message
    PrepareInputMessage,
}

/// Runs at the start of every FixedUpdate schedule
/// The USER must write this. Check what inputs were pressed and add them to the input buffer
/// DO NOT RUN THIS DURING ROLLBACK
fn update_input_buffer<P: Protocol>(mut client: ResMut<Client<P>>) {}

// system that runs after update_input_buffer, and uses the input to update the world?
// - NOT rollback: gets the input for the current tick from the input buffer, only runs on predicted entities.
// - IN rollback: gets the input for the current rollback tick from the input buffer, only runs on predicted entities.
fn apply_input() {}

// Take the input buffer, and prepare the input message to send to the server
fn prepare_input_message<P: Protocol>(mut client: ResMut<Client<P>>) {
    // TODO: the number of messages should be in SharedConfig
    trace!(tick = ?client.tick(), "prepare_input_message");
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
