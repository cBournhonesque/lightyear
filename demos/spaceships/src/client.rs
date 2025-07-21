use avian2d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::core::timeline::is_in_rollback;
use lightyear::input::input_buffer::InputBuffer;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

use crate::protocol::*;
use crate::shared::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // all actions related-system that can be rolled back should be in FixedUpdate schedule
        app.add_systems(FixedUpdate, (player_movement, shared_player_firing).chain());
        app.add_observer(add_ball_physics);
        app.add_observer(add_bullet_physics);
        app.add_observer(handle_new_player);

        app.add_systems(
            FixedUpdate,
            handle_hit_event
                .run_if(on_event::<BulletHitEvent>)
                .after(process_collisions),
        );
    }
}

/// When the ball gets replicated from the server, add all the components
/// that we need that are not replicated.
/// (for example physical properties that are constant, so they don't need to be networked)
///
/// We only add the physical properties on the ball that is displayed on screen (i.e the Predicted ball)
/// We want the ball to be rigid so that when players collide with it, they bounce off.
fn add_ball_physics(
    trigger: Trigger<OnAdd, BallMarker>,
    ball_query: Query<&BallMarker, With<Predicted>>,
    mut commands: Commands,
) {
    let entity = trigger.target();
    if let Ok(ball) = ball_query.get(entity) {
        info!("Adding physics to a replicated ball {entity:?}");
        commands.entity(entity).insert(ball.physics_bundle());
    }
}

/// Simliar blueprint scenario as balls, except sometimes clients prespawn bullets ahead of server
/// replication, which means they will already have the physics components.
/// So, we filter the query using `Without<Collider>`.
fn add_bullet_physics(
    trigger: Trigger<OnAdd, BulletMarker>,
    mut commands: Commands,
    bullet_query: Query<(), (With<Predicted>, Without<Collider>)>,
) {
    let entity = trigger.target();
    if let Ok(()) = bullet_query.get(entity) {
        info!("Adding physics to a replicated bullet: {entity:?}");
        commands.entity(entity).insert(PhysicsBundle::bullet());
    }
}

/// Decorate newly connecting players with physics components
/// ..and if it's our own player, set up input stuff
fn handle_new_player(
    trigger: Trigger<OnAdd, (Player, Predicted)>,
    mut commands: Commands,
    player_query: Query<(&Player, Has<Controlled>), With<Predicted>>,
) {
    let entity = trigger.target();
    if let Ok((player, is_controlled)) = player_query.get(entity) {
        info!("handle_new_player, entity = {entity:?} is_controlled = {is_controlled}");
        // is this our own entity?
        if is_controlled {
            info!("Own player replicated to us, adding inputmap {entity:?} {player:?}");
            commands.entity(entity).insert(InputMap::new([
                (PlayerActions::Up, KeyCode::ArrowUp),
                (PlayerActions::Down, KeyCode::ArrowDown),
                (PlayerActions::Left, KeyCode::ArrowLeft),
                (PlayerActions::Right, KeyCode::ArrowRight),
                (PlayerActions::Up, KeyCode::KeyW),
                (PlayerActions::Down, KeyCode::KeyS),
                (PlayerActions::Left, KeyCode::KeyA),
                (PlayerActions::Right, KeyCode::KeyD),
                (PlayerActions::Fire, KeyCode::Space),
            ]));
        } else {
            info!("Remote player replicated to us: {entity:?} {player:?}");
        }
        commands.entity(entity).insert(PhysicsBundle::player_ship());
    }
}

// Generate an explosion effect for bullet collisions
fn handle_hit_event(
    time: Res<Time>,
    mut events: EventReader<BulletHitEvent>,
    mut commands: Commands,
) {
    for ev in events.read() {
        commands.spawn((
            Transform::from_xyz(ev.position.x, ev.position.y, 0.0),
            Visibility::default(),
            crate::renderer::Explosion::new(time.elapsed(), ev.bullet_color),
        ));
    }
}

// only apply movements to predicted entities
fn player_movement(
    mut q: Query<(&ActionState<PlayerActions>, ApplyInputsQuery), (With<Player>, With<Predicted>)>,
    timeline: Single<&LocalTimeline, With<PredictionManager>>,
) {
    // get the tick, even if during rollback
    let tick = timeline.tick();

    for (action_state, mut aiq) in q.iter_mut() {
        if !action_state.get_pressed().is_empty() {
            trace!(
                "🎹 {:?} {tick:?} = {:?}",
                aiq.player.client_id,
                action_state.get_pressed(),
            );
        }
        apply_action_state_to_player_movement(action_state, &mut aiq, tick);
    }
}
