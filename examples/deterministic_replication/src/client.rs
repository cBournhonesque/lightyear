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
    client: Single<&LocalId, With<Client>>,
    timeline: Res<LocalTimeline>,
    mut commands: Commands,
    player_query: Query<&PlayerId>,
) {
    let local_id = client.into_inner();
    let tick = timeline.tick();

    // store the tick when the game started, so we can remove the DisableRollback component later
    let peer_id = player_query.get(trigger.entity).unwrap().0;
    info!("Received player spawn for player {peer_id:?} at tick {tick:?}");
    let mut entity_mut = commands.entity(trigger.entity);
    entity_mut.insert((
        player_bundle(peer_id),
        // this indicates that the entity will only do rollbacks from input updates, and not state updates!
        // It is REQUIRED to add this component to indicate which entities will be rollbacked
        // in deterministic replication mode.
        DeterministicPredicted {
            // any rollback would try to reset this entity to the rollback tick. If the entity was spawned after the rollback tick,
            // it would get despawned instantly. For entities that are spawned via a one-off event, we can mark them as
            // `skip_despawn` which will temporarily disable this entity from rollbacks for a few ticks after being spawned.
            skip_despawn: true,
            ..default()
        },
    ));
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
