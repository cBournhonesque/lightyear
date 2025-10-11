use avian2d::parry::shape::Ball;
use avian2d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::prediction::predicted_history::PredictionHistory;
use lightyear::prediction::rollback::{DeterministicPredicted, DisableRollback};
use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;
use crate::shared::{SharedPlugin, color_from_id, player_bundle, shared_movement_behaviour};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(handle_player_spawn);
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - add physics components so that its movement can be predicted
/// When we receive a new player entity
fn handle_player_spawn(
    trigger: On<Add, PlayerId>,
    client: Single<(&LocalId, &LocalTimeline), With<Client>>,
    mut commands: Commands,
    player_query: Query<&PlayerId>,
) {
    let (local_id, timeline) = client.into_inner();
    let tick = timeline.tick();

    // store the tick when the game started, so we can remove the DisableRollback component later
    let peer_id = player_query.get(trigger.entity).unwrap().0;
    info!("Received player spawn for player {peer_id:?} at tick {tick:?}");
    let mut entity_mut = commands.entity(trigger.entity);
    entity_mut.insert(player_bundle(peer_id));
    // keep track of when the entity was spawned
    if local_id.0 == peer_id {
        entity_mut.insert(InputMap::new([
            (PlayerActions::Up, KeyCode::KeyW),
            (PlayerActions::Down, KeyCode::KeyS),
            (PlayerActions::Left, KeyCode::KeyA),
            (PlayerActions::Right, KeyCode::KeyD),
        ]));
    }
}
