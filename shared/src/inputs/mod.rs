use bevy::prelude::Plugin;

/// Handles dealing with inputs (keyboard presses, mouse clicks) sent from a player (client) to server
mod input_buffer;
mod plugin;

// on the client:
// - FixedUpdate: before physics but after increment tick,
//   - rollback: we get the input from the history
//   - we get the input from keyboard/mouse and store it in the InputBuffer
//   - can use system piping?
// - Send:
//   - we read the
pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_resource(InputBuffer::<Input>::default())
            .add_system_set(
                SystemSet::on_update(AppState::Game)
                    .with_system(input_system.system())
                    .with_system(send_inputs.system()),
            );
    }
}

// on the server:
// - When we receive an input, we
