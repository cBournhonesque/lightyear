use avian2d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::prediction::rollback::{DeterministicPredicted, DisableRollback};
use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;
use crate::shared::{SharedPlugin, color_from_id, shared_movement_behaviour};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, (player_movement, handle_game_start));
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
    let y = (peer_id.to_bits() as f32 * 50.0) % 500.0 - 250.0;
    let color = color_from_id(peer_id);
    let mut entity_mut = commands.entity(trigger.target());
    entity_mut.insert((
        Position::from(Vec2::new(-50.0, y)),
        ColorComponent(color),
        PhysicsBundle::player(),
        Name::from("Player"),
        // this indicates that the entity will only do rollbacks from input updates, and not state updates!
        // It is currently REQUIRED to add this component to indicate which entities will be rollbacked
        // in deterministic replication mode.
        DeterministicPredicted,
        // this is a bit subtle:
        // when we add DeterministicPredicted to the entity, we enable it for rollbacks. Since we have RollbackMode::Always,
        // we will try to rollback on every input received. We will therefore rollback to before the entity was spawned,
        // which will immediately despawn the entity!
        // This is because we are not creating the entity in a deterministic way. (if we did, we would be re-creating the
        // entity during the rollbacks). As a workaround, we disable rollbacks for this entity for a few ticks.
        DisableRollback,
    ));
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
