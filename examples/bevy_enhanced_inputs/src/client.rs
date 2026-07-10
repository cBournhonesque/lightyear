//! The client plugin.
//! The client will be responsible for:
//! - connecting to the server at Startup
//! - sending inputs to the server
//! - applying inputs to the locally predicted player (for prediction to work, inputs have to be applied to both the
//!   predicted entity and the server entity)

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared;
use bevy::ecs::relationship::Relationship;
use bevy::prelude::*;
#[cfg(feature = "server")]
use lightyear::connection::host::HostServer;
use lightyear::input::bei::prelude::{Action, ActionOf, Actions, Bindings, Cardinal, Fire};
use lightyear::prelude::client::{InputDelayConfig, InputTimelineConfig};
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        app.add_systems(Startup, configure_input_delay);
        app.add_observer(add_bindings_to_controlled_action);
        app.add_observer(add_bindings_to_controlled_player_actions);
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_interpolated_spawn);
        app.add_observer(player_movement);
    }
}

fn configure_input_delay(client: Single<Entity, With<Client>>, mut commands: Commands) {
    commands
        .entity(client.into_inner())
        .insert(InputTimelineConfig::default().with_input_delay(InputDelayConfig::balanced()));
}

/// Applies local movement only to predicted entities owned by this client.
/// In host-server mode the server observer runs in the same app, so the client
/// observer is disabled.
fn player_movement(
    trigger: On<Fire<Movement>>,
    synced_client: Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    #[cfg(feature = "server")] host_server: Query<(), With<HostServer>>,
    mut position_query: Query<&mut PlayerPosition, With<Predicted>>,
) {
    if synced_client.is_empty() {
        return;
    }
    #[cfg(feature = "server")]
    if !host_server.is_empty() {
        return;
    }
    if let Ok(position) = position_query.get_mut(trigger.context) {
        // Pass Mut<PlayerPosition> directly so change detection only fires when movement changes it.
        shared::shared_movement_behaviour(position, trigger.value);
    }
}

/// Lower the saturation on predicted entities so they are visually distinct.
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, (PlayerId, Predicted)>,
    mut predicted: Query<(&PlayerId, &mut PlayerColor), With<Predicted>>,
) {
    let entity = trigger.entity;
    if let Ok((_player_id, mut color)) = predicted.get_mut(entity) {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

/// Add local movement bindings once the replicated action entity is fully
/// ready for a player that we control.
fn add_bindings_to_controlled_action(
    trigger: On<Insert, (Action<Movement>, ActionOf<Player>)>,
    actions: Query<&ActionOf<Player>, (With<Action<Movement>>, Without<Bindings>)>,
    controlled_players: Query<(), (With<Player>, With<Controlled>)>,
    mut commands: Commands,
) {
    let Ok(action_of) = actions.get(trigger.entity) else {
        return;
    };
    if controlled_players.get(action_of.get()).is_err() {
        return;
    }
    commands
        .entity(trigger.entity)
        .insert(Bindings::spawn(Cardinal::wasd_keys()));
}

/// Add local movement bindings if the controlled player becomes ready after
/// its action entities.
fn add_bindings_to_controlled_player_actions(
    trigger: On<Add, (Player, Controlled, Actions<Player>)>,
    players: Query<&Actions<Player>, (With<Player>, With<Controlled>)>,
    actions: Query<(), (With<Action<Movement>>, Without<Bindings>)>,
    mut commands: Commands,
) {
    let Ok(player_actions) = players.get(trigger.entity) else {
        return;
    };
    for action in player_actions.iter() {
        if actions.get(action).is_err() {
            continue;
        }
        commands
            .entity(action)
            .insert(Bindings::spawn(Cardinal::wasd_keys()));
    }
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
