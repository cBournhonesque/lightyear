use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::utils::Duration;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::*;
use leafwing_input_manager::systems::{run_if_enabled, tick_action_state};

use lightyear::_reexport::ShouldBeInterpolated;
pub use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared::{shared_config, shared_movement_behaviour};
use crate::{shared, Cli, ClientTransports, SharedSettings};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LeafwingInputPlugin::<MyProtocol, Inputs>::default());
        app.init_resource::<ActionState<Inputs>>();
        app.add_systems(Startup, init);
        app.add_systems(PreUpdate, handle_connection.after(MainSet::Receive));
        app.add_systems(FixedUpdate, movement);
        app.add_systems(
            Update,
            (
                add_input_map,
                handle_predicted_spawn,
                handle_interpolated_spawn,
            ),
        );
    }
}

/// Startup system for the client
pub(crate) fn init(mut commands: Commands) {
    commands.connect_client();
}

/// Listen for events to know when the client is connected, and spawn a text entity
/// to display the client id
pub(crate) fn handle_connection(
    mut commands: Commands,
    mut connection_event: EventReader<ConnectEvent>,
) {
    for event in connection_event.read() {
        let client_id = event.client_id();
        commands.spawn(TextBundle::from_section(
            format!("Client {}", client_id),
            TextStyle {
                font_size: 30.0,
                color: Color::WHITE,
                ..default()
            },
        ));
    }
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
pub(crate) fn movement(
    // TODO: maybe make prediction mode a separate component!!!
    mut position_query: Query<(&mut Position, &ActionState<Inputs>), With<Predicted>>,
) {
    // if we are not doing prediction, no need to read inputs
    if <Components as SyncMetadata<Position>>::mode() != ComponentSyncMode::Full {
        return;
    }
    for (position, input) in position_query.iter_mut() {
        shared_movement_behaviour(position, input);
    }
}

// System to receive messages on the client
pub(crate) fn add_input_map(
    mut commands: Commands,
    predicted_players: Query<Entity, (Added<PlayerId>, With<Predicted>)>,
) {
    // we don't want to replicate the ActionState from the server to client, because if we have an ActionState
    // on the Confirmed player it will keep getting replicated to Predicted and will interfere with our inputs
    for player_entity in predicted_players.iter() {
        commands.entity(player_entity).insert((
            PlayerBundle::get_input_map(),
            ActionState::<Inputs>::default(),
        ));
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
pub(crate) fn handle_predicted_spawn(mut predicted: Query<&mut PlayerColor, Added<Predicted>>) {
    for mut color in predicted.iter_mut() {
        color.0.set_s(0.3);
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
pub(crate) fn handle_interpolated_spawn(
    mut interpolated: Query<&mut PlayerColor, Added<Interpolated>>,
) {
    for mut color in interpolated.iter_mut() {
        color.0.set_s(0.1);
    }
}
