use avian2d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::utils::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::inputs::leafwing::input_buffer::InputBuffer;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear::shared::replication::components::Controlled;
use lightyear::shared::tick_manager;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

use crate::protocol::*;
use crate::shared::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // all actions related-system that can be rolled back should be in FixedUpdate schedule
        app.add_systems(
            FixedUpdate,
            (
                // in host-server, we don't want to run the movement logic twice
                // disable this because we also run the movement logic in the server
                player_movement.run_if(not(is_host_server)),
                // we don't spawn bullets during rollback.
                // if we have the inputs early (so not in rb) then we spawn,
                // otherwise we rely on normal server replication to spawn them
                shared_player_firing.run_if(not(is_in_rollback)),
            )
                .chain()
                .in_set(FixedSet::Main),
        );
        app.add_systems(
            Update,
            (
                add_ball_physics,
                add_bullet_physics, // TODO better to scheduled right after replicated entities get spawned?
                handle_new_player,
            ),
        );
        app.add_systems(
            FixedUpdate,
            handle_hit_event
                .run_if(on_event::<BulletHitEvent>())
                .after(process_collisions),
        );
    }
}

/// Blueprint pattern: when the ball gets replicated from the server, add all the components
/// that we need that are not replicated.
/// (for example physical properties that are constant, so they don't need to be networked)
///
/// We only add the physical properties on the ball that is displayed on screen (i.e the Predicted ball)
/// We want the ball to be rigid so that when players collide with it, they bounce off.
fn add_ball_physics(
    mut commands: Commands,
    mut ball_query: Query<(Entity, &BallMarker), Added<Predicted>>,
) {
    for (entity, ball) in ball_query.iter_mut() {
        info!("Adding physics to a replicated ball {entity:?}");
        commands.entity(entity).insert(ball.physics_bundle());
    }
}

/// Simliar blueprint scenario as balls, except sometimes clients prespawn bullets ahead of server
/// replication, which means they will already have the physics components.
/// So, we filter the query using `Without<Collider>`.
fn add_bullet_physics(
    mut commands: Commands,
    mut bullet_query: Query<Entity, (With<BulletMarker>, Added<Predicted>, Without<Collider>)>,
) {
    for entity in bullet_query.iter_mut() {
        info!("Adding physics to a replicated bullet:  {entity:?}");
        commands.entity(entity).insert(PhysicsBundle::bullet());
    }
}

/// Decorate newly connecting players with physics components
/// ..and if it's our own player, set up input stuff
fn handle_new_player(
    connection: Res<ClientConnection>,
    mut commands: Commands,
    mut player_query: Query<(Entity, &Player, Has<Controlled>), Added<Predicted>>,
) {
    for (entity, player, is_controlled) in player_query.iter_mut() {
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
        let client_id = connection.id();
        info!(?entity, ?client_id, "adding physics to predicted player");
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
            SpatialBundle {
                transform: Transform::from_xyz(ev.position.x, ev.position.y, 0.0),
                ..default()
            },
            crate::renderer::Explosion::new(time.elapsed(), ev.bullet_color),
        ));
    }
}

// only apply movements to predicted entities
fn player_movement(
    mut q: Query<
        (
            &ActionState<PlayerActions>,
            &InputBuffer<PlayerActions>,
            ApplyInputsQuery,
        ),
        (With<Player>, With<Predicted>),
    >,
    tick_manager: Res<TickManager>,
    rollback: Option<Res<Rollback>>,
) {
    // max number of stale inputs to predict before default inputs used
    const MAX_STALE_TICKS: u16 = 6;
    // get the tick, even if during rollback
    let tick = rollback
        .as_ref()
        .map(|rb| tick_manager.tick_or_rollback_tick(rb))
        .unwrap_or(tick_manager.tick());

    for (action_state, input_buffer, mut aiq) in q.iter_mut() {
        // is the current ActionState for real?
        if input_buffer.get(tick).is_some() {
            // Got an exact input for this tick, staleness = 0, the happy path.
            apply_action_state_to_player_movement(action_state, 0, &mut aiq, tick);
            continue;
        }

        // if the true input is missing, this will be leftover from a previous tick, or the default().
        if let Some((prev_tick, prev_input)) = input_buffer.get_last_with_tick() {
            let staleness = (tick - prev_tick).max(0) as u16;
            if staleness > MAX_STALE_TICKS {
                // input too stale, apply default input (ie, nothing pressed)
                apply_action_state_to_player_movement(
                    &ActionState::default(),
                    staleness,
                    &mut aiq,
                    tick,
                );
            } else {
                // apply a stale input within our acceptable threshold.
                // we could use the staleness to decay movement forces as desired.
                apply_action_state_to_player_movement(prev_input, staleness, &mut aiq, tick);
            }
        } else {
            // no inputs in the buffer yet, can happen during initial connection.
            // apply the default input (ie, nothing pressed)
            apply_action_state_to_player_movement(action_state, 0, &mut aiq, tick);
        }
    }
}
