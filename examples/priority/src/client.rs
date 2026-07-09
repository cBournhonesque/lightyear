use bevy::ecs::relationship::Relationship;
use bevy::prelude::*;
use lightyear::connection::host::HostServer;
use lightyear::input::bei::prelude::{Action, ActionOf, Bindings, Cardinal, Fire};
use lightyear::prelude::client::{InputDelayConfig, InputTimelineConfig};
use lightyear::prelude::*;

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        app.add_systems(Startup, configure_input_delay);
        app.add_observer(player_movement);
        app.add_observer(handle_interpolated_spawn);
        app.add_observer(handle_predicted_spawn);
        app.add_observer(add_bindings_to_controlled_actions);
    }
}

fn configure_input_delay(client: Single<Entity, With<Client>>, mut commands: Commands) {
    commands
        .entity(client.into_inner())
        .insert(InputTimelineConfig::default().with_input_delay(InputDelayConfig::balanced()));
}

/// Applies local movement only to predicted entities owned by this client.
fn player_movement(
    trigger: On<Fire<Movement>>,
    synced_client: Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    _host_server: Query<(), With<HostServer>>,
    #[cfg(feature = "server")] server_actions: Query<
        (),
        (With<Action<Movement>>, With<crate::server::ServerAction>),
    >,
    mut position_query: Query<&mut Position, With<Predicted>>,
) {
    if synced_client.is_empty() {
        return;
    }
    #[cfg(feature = "server")]
    if !_host_server.is_empty() && server_actions.contains(trigger.action) {
        return;
    }
    if let Ok(position) = position_query.get_mut(trigger.context) {
        shared::shared_movement_behaviour(position, trigger.value);
    }
}

/// Lower the saturation on predicted entities so they are visually distinct.
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, (PlayerId, Predicted)>,
    mut predicted: Query<&mut PlayerColor, With<Predicted>>,
) {
    let entity = trigger.entity;
    if let Ok(mut color) = predicted.get_mut(entity) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

/// Add local movement bindings when we receive an Action entity for a player
/// that we control.
pub(crate) fn add_bindings_to_controlled_actions(
    trigger: On<Add, (Action<Movement>, ActionOf<Player>)>,
    actions: Query<(&ActionOf<Player>, Has<Bindings>), With<Action<Movement>>>,
    controlled_players: Query<(), (With<Player>, With<Controlled>)>,
    mut commands: Commands,
) {
    let Ok((action_of, has_bindings)) = actions.get(trigger.entity) else {
        return;
    };
    if has_bindings || controlled_players.get(action_of.get()).is_err() {
        return;
    }
    commands
        .entity(trigger.entity)
        .insert(Bindings::spawn((Cardinal::wasd_keys(), Cardinal::arrows())));
}

/// Lower the saturation on interpolated entities so they are visually distinct.
pub(crate) fn handle_interpolated_spawn(
    trigger: On<Add, PlayerColor>,
    mut interpolated: Query<&mut PlayerColor, With<Interpolated>>,
) {
    if let Ok(mut color) = interpolated.get_mut(trigger.entity) {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}
