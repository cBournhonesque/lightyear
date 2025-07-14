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
        app.add_systems(FixedUpdate, handle_game_start);
        app.add_observer(handle_player_spawn);
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - add physics components so that its movement can be predicted
/// When we receive a new player entity
fn handle_player_spawn(
    trigger: Trigger<OnAdd, PlayerId>,
    client: Single<(&LocalId, &LocalTimeline), With<Client>>,
    mut commands: Commands,
    player_query: Query<&PlayerId>,
) {
    let (local_id, timeline) = client.into_inner();
    let tick = timeline.tick();

    // store the tick when the game started, so we can remove the DisableRollback component later
    commands.insert_resource(GameStart(tick));
    info!("Received GameStart at tick: {tick:?}");

    let peer_id = player_query.get(trigger.target()).unwrap().0;
    let mut entity_mut = commands.entity(trigger.target());
    entity_mut.insert(player_bundle(peer_id));
    if local_id.0 == peer_id {
        entity_mut.insert(InputMap::new([
            (PlayerActions::Up, KeyCode::KeyW),
            (PlayerActions::Down, KeyCode::KeyS),
            (PlayerActions::Left, KeyCode::KeyA),
            (PlayerActions::Right, KeyCode::KeyD),
        ]));
    }
}

#[derive(Resource)]
struct GameStart(Tick);

/// Remove the DisableRollback component from all entities a little bit after the game started.
fn handle_game_start(
    timeline: Single<&LocalTimeline, With<Client>>,
    query: Query<Entity, With<DisableRollback>>,
    mut commands: Commands,
    resource: Option<Res<GameStart>>,
) {
    if let Some(res) = resource
        && timeline.tick() >= res.0 + 20
    {
        info!("Time to remove DisableRollback");
        query.iter().for_each(|e| {
            info!("Removed DisableRollback from entity: {:?}", e);
            commands.entity(e).remove::<DisableRollback>();
        });
        commands.remove_resource::<GameStart>();
    }
}
