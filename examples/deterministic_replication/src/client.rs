use avian2d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::prediction::rollback::DisableStateRollback;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_movement_behaviour, SharedPlugin};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, player_movement);
        app.add_observer(handle_player_spawn);
    }
}

/// In deterministic replication, the client simulates all players.
fn player_movement(
    timeline: Single<&LocalTimeline, With<Client>>,
    mut velocity_query: Query<(
        Entity,
        &PlayerId,
        &Position,
        &mut LinearVelocity,
        &ActionState<PlayerActions>,
    )>,
) {
    let tick = timeline.tick();
    for (entity, player_id, position, velocity, action_state) in velocity_query.iter_mut() {
        if !action_state.get_pressed().is_empty() {
            trace!(?entity, ?tick, ?position, actions = ?action_state.get_pressed(), "applying movement to predicted player");
            // note that we also apply the input to the other predicted clients! even though
            //  their inputs are only replicated with a delay!
            // TODO: add input decay?
            shared_movement_behaviour(velocity, action_state);
        }
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - add physics components so that its movement can be predicted
/// When we receive a new player entity
pub(crate) fn handle_player_spawn(
    trigger: Trigger<OnAdd, PlayerId>,
    client: Single<&LocalId, With<Client>>,
    mut commands: Commands,
    player_query: Query<&PlayerId>,
) {
    let local_id = client.0;
    let peer_id = player_query.get(trigger.target()).unwrap().0;
    let y = (peer_id.to_bits() as f32 * 50.0) % 500.0 - 250.0;
    let color = color_from_id(peer_id);
    let mut entity_mut = commands.entity(trigger.target());
    entity_mut.insert((
        Position::from(Vec2::new(-50.0, y)),
        ColorComponent(color),
        PhysicsBundle::player(),
        Name::from("Player"),
        // this indicates that the entity will only do rollbacks from input updates, and not state updates
        // It is currently REQUIRED to add this component to indicate which entities will be rollbacked
        // in deterministic replication mode.
        DisableStateRollback,
    ));
    if local_id == peer_id {
        entity_mut.insert(InputMap::new([
            (PlayerActions::Up, KeyCode::KeyW),
            (PlayerActions::Down, KeyCode::KeyS),
            (PlayerActions::Left, KeyCode::KeyA),
            (PlayerActions::Right, KeyCode::KeyD),
        ]));
    }
}
