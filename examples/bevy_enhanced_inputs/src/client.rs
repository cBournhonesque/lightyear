//! The client plugin.
//! The client will be responsible for:
//! - connecting to the server at Startup
//! - sending inputs to the server
//! - applying inputs to the locally predicted player (for prediction to work, inputs have to be applied to both the
//!   predicted entity and the server entity)

use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared;
use bevy::prelude::*;
use lightyear::input::bei::prelude::Fire;
use lightyear::prelude::input::bei::InputMarker;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_controlled_spawn);
        app.add_observer(handle_interpolated_spawn);
        app.add_observer(player_movement);
    }
}

/// The client input only gets applied to predicted entities that we own
/// This works because we only predict the user's controlled entity.
/// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    trigger: On<Fire<Movement>>,
    mut position_query: Query<&mut PlayerPosition, With<Predicted>>,
) {
    if let Ok(position) = position_query.get_mut(trigger.context) {
        // NOTE: be careful to directly pass Mut<PlayerPosition>
        // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
        shared::shared_movement_behaviour(position, trigger.value);
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
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

/// Spawn local action entities once the local player is actually controlled by this client.
///
/// We intentionally key this off `Add<Controlled>`, not `Add<Predicted>`.
/// In dedicated client/server mode the replicated entity often ends up with both markers,
/// but in host-server mode the local entity is created in the same world through local
/// insertion paths, not through Replicon's deferred receive bundle. In that mode there is no
/// reliable guarantee that `Controlled` will already exist when `Predicted` is added, so
/// checking `Has<Controlled>` inside `Add<Predicted>` is brittle.
///
/// `Controlled` is the actual semantic signal we care about for local input setup: once this
/// marker appears, the entity belongs to this local client and it is safe to attach the
/// `InputMarker` and spawn the local BEI action entities.
fn handle_controlled_spawn(
    trigger: On<Add, Controlled>,
    controlled_players: Query<
        (&PlayerId, Has<InputMarker<Player>>),
        With<Player>,
    >,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok((player_id, has_input_marker)) = controlled_players.get(entity) else {
        return;
    };
    if has_input_marker {
        return;
    }
    commands
        .entity(entity)
        .insert(InputMarker::<Player>::default());
    shared::spawn_action_entities(&mut commands, entity, player_id.0, false);
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
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
