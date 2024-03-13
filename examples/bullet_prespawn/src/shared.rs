use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::utils::Duration;
use bevy_screen_diagnostics::{Aggregate, ScreenDiagnostics, ScreenDiagnosticsPlugin};
use leafwing_input_manager::orientation::Orientation;
use leafwing_input_manager::prelude::ActionState;
use tracing::Level;

use lightyear::client::prediction::plugin::is_in_rollback;
use lightyear::client::prediction::{Rollback, RollbackState};
use lightyear::prelude::client::*;
use lightyear::prelude::TickManager;
use lightyear::prelude::*;
use lightyear::transport::io::IoDiagnosticsPlugin;

use crate::protocol::*;

const FRAME_HZ: f64 = 60.0;
const FIXED_TIMESTEP_HZ: f64 = 64.0;

const EPS: f32 = 0.0001;

pub fn shared_config() -> SharedConfig {
    SharedConfig {
        client_send_interval: Duration::default(),
        server_send_interval: Duration::from_secs_f64(1.0 / 32.0),
        // server_send_interval: Duration::from_millis(500),
        tick: TickConfig {
            tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        },
    }
}

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        if app.is_plugin_added::<RenderPlugin>() {
            // draw after interpolation is done
            app.add_systems(
                PostUpdate,
                draw_elements
                    .after(InterpolationSet::Interpolate)
                    .after(PredictionSet::VisualCorrection),
            );
            // app.add_plugins(LogDiagnosticsPlugin {
            //     filter: Some(vec![
            //         IoDiagnosticsPlugin::BYTES_IN,
            //         IoDiagnosticsPlugin::BYTES_OUT,
            //     ]),
            //     ..default()
            // });
            app.add_systems(Startup, setup_diagnostic);
            app.add_plugins(ScreenDiagnosticsPlugin::default());
        }

        // registry types for reflection
        app.register_type::<PlayerId>();
        app.add_systems(FixedPostUpdate, fixed_update_log);
        // every system that is physics-based and can be rolled-back has to be in the `FixedUpdate` schedule
        app.add_systems(
            FixedUpdate,
            // ideally, during rollback, we'd despawn the pre-predicted player objects and then respawn them during shoot_bullet.
            // how? we keep track of their spawn-tick, if it was before the rollback-tick we despawn.
            //  - for every pre-predicted or pre-spawned entity, we keep track of the spawn tick.
            //  - if we rollback to before that, we
            (
                player_movement,
                shoot_bullet,
                // shoot_bullet.run_if(not(is_in_rollback)),
                move_bullet,
            )
                .chain(),
        );
        // NOTE: we need to create prespawned entities in FixedUpdate, because only then are inputs correctly associated with a tick
        //  Example:
        //  tick = 0
        //   F1 PreUpdate: press-shoot. F1 FixedUpdate: SKIPPED!!! F1 Update: spawn bullet F1: PostUpdate add hash.
        //   F2 FixedUpdate: tick = 1. Gather inputs for the tick (i.e. F1 preupdate + F2 preupdate)
        //  So now the server will think that the bullet was shot at tick = 1, but on client it was shot on tick = 0 and the hashes won't match.
        //  In general, most input-handling needs to be handled in FixedUpdate to be correct.
        // app.add_systems(Update, shoot_bullet);
    }
}

fn setup_diagnostic(mut onscreen: ResMut<ScreenDiagnostics>) {
    onscreen
        .add("KB/S in".to_string(), IoDiagnosticsPlugin::BYTES_IN)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:.2}"));
    onscreen
        .add("KB/s out".to_string(), IoDiagnosticsPlugin::BYTES_OUT)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:.2}"));
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: ClientId) -> Color {
    let h = (((client_id.wrapping_mul(90)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_player_movement(
    mut transform: Mut<Transform>,
    action: &ActionState<PlayerActions>,
) {
    const PLAYER_MOVE_SPEED: f32 = 10.0;
    // warn!(?action, "action state");
    let Some(cursor_data) = action.action_data(&PlayerActions::MoveCursor) else {
        return;
    };
    let mouse_position = cursor_data
        .axis_pair
        .map(|axis| axis.xy())
        .unwrap_or_default();
    // warn!(?mouse_position);
    let angle =
        Vec2::new(0.0, 1.0).angle_between(mouse_position - transform.translation.truncate());
    // careful to only activate change detection if there was an actual change
    if (angle - transform.rotation.to_euler(EulerRot::XYZ).2).abs() > EPS {
        transform.rotation = Quat::from_rotation_z(angle);
    }
    // TODO: look_at should work
    // transform.look_at(Vec3::new(mouse_position.x, mouse_position.y, 0.0), Vec3::Y);
    if action.pressed(&PlayerActions::Up) {
        transform.translation.y += PLAYER_MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Down) {
        transform.translation.y -= PLAYER_MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Right) {
        transform.translation.x += PLAYER_MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Left) {
        transform.translation.x -= PLAYER_MOVE_SPEED;
    }
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    tick_manager: Res<TickManager>,
    mut player_query: Query<
        (&mut Transform, &ActionState<PlayerActions>, &PlayerId),
        (Without<Confirmed>, Without<Interpolated>),
    >,
) {
    for (transform, action_state, player_id) in player_query.iter_mut() {
        shared_player_movement(transform, action_state);
        // info!(tick = ?tick_manager.tick(), ?transform, actions = ?action_state.get_pressed(), "applying movement to predicted player");
    }
}

pub(crate) fn fixed_update_log(
    tick_manager: Res<TickManager>,
    rollback: Option<Res<Rollback>>,
    player: Query<(Entity, &Transform), (With<PlayerId>, Without<Confirmed>)>,
    ball: Query<(Entity, &Transform), (With<BallMarker>, Without<Confirmed>)>,
    interpolated_ball: Query<(Entity, &Transform), (With<BallMarker>, With<Interpolated>)>,
) {
    let mut tick = tick_manager.tick();
    if let Some(rollback) = rollback {
        if let RollbackState::ShouldRollback { current_tick } = rollback.state {
            tick = current_tick;
        }
    }
    for (entity, transform) in player.iter() {
        trace!(
        ?tick,
        ?entity,
        pos = ?transform.translation.truncate(),
        "Player after fixed update"
        );
    }
    for (entity, transform) in ball.iter() {
        trace!(
            ?tick,
            ?entity,
            pos = ?transform.translation.truncate(),
            "Ball after fixed update"
        );
    }
    for (entity, transform) in interpolated_ball.iter() {
        trace!(
            ?tick,
            ?entity,
            pos = ?transform.translation.truncate(),
            "interpolated Ball after fixed update"
        );
    }
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn move_bullet(
    mut commands: Commands,
    mut query: Query<
        (Entity, &mut Transform),
        (With<BallMarker>, Without<Confirmed>, Without<Interpolated>),
    >,
) {
    const BALL_MOVE_SPEED: f32 = 3.0;
    const MAP_LIMIT: f32 = 2000.0;
    for (entity, mut transform) in query.iter_mut() {
        let movement_direction = transform.rotation * Vec3::Y;
        transform.translation += movement_direction * BALL_MOVE_SPEED;
        // destroy bullets that are out of the screen
        if transform.translation.x.abs() > MAP_LIMIT || transform.translation.y.abs() > MAP_LIMIT {
            // TODO: use the predicted despawn?
            commands.entity(entity).despawn();
        }
    }
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shoot_bullet(
    mut commands: Commands,
    tick_manager: Res<TickManager>,
    identity: NetworkIdentity,
    mut query: Query<
        (
            &PlayerId,
            &Transform,
            &ColorComponent,
            &mut ActionState<PlayerActions>,
        ),
        (Without<Interpolated>, Without<Confirmed>),
    >,
) {
    let tick = tick_manager.tick();
    const BALL_MOVE_SPEED: f32 = 10.0;
    for (id, transform, color, mut action) in query.iter_mut() {
        // NOTE: cannot use FixedUpdate + JustPressed in case we have a frame with no FixedUpdate, then the JustPressed would be lost
        // NOTE: cannot use Update + JustPressed, because in case we have a frame with no FixedUpdate, the action-diff would be sent for
        //  a different diff than the one where the action was executed. (but maybe we can account for that)
        // NOTE: cannot spawn the bullet during FixedUpdate because then during rollback we spawn a new bullet! For now just set the system to
        //  run when not in rollback
        // TODO:  if we were running this in FixedUpdate, we would need to `consume` the action. (in case there are several fixed-update steps
        //  ine one frame). We also cannot use JustPressed, because we could have a frame with no FixedUpdate.
        // NOTE: pressed lets you shoot many bullets, which can be cool
        if action.pressed(&PlayerActions::Shoot) {
            action.consume(&PlayerActions::Shoot);

            info!(?tick, pos=?transform.translation.truncate(), rot=?transform.rotation.to_euler(EulerRot::XYZ).2, "spawn bullet");

            for delta in &[-0.2, 0.2] {
                // for delta in &[0.0] {
                let ball = BallBundle::new(
                    transform.translation.truncate(),
                    transform.rotation.to_euler(EulerRot::XYZ).2 + delta,
                    color.0,
                    false,
                );
                // on the server, replicate the bullet
                if identity.is_server() {
                    commands.spawn((
                        ball,
                        // NOTE: the PreSpawnedPlayerObject component indicates that the entity will be spawned on both client and server
                        //  but the server will take authority as soon as the client receives the entity
                        //  it does this by matching with the client entity that has the same hash
                        //  The hash is computed automatically in PostUpdate from the entity's components + spawn tick
                        //  unless you set the hash manually before PostUpdate to a value of your choice
                        PreSpawnedPlayerObject::default(),
                        Replicate {
                            replication_target: NetworkTarget::All,
                            // the bullet is predicted for the client who shot it
                            prediction_target: NetworkTarget::Single(id.0),
                            // the bullet is interpolated for other clients
                            interpolation_target: NetworkTarget::AllExceptSingle(id.0),
                            // NOTE: all predicted entities need to have the same replication group
                            replication_group: ReplicationGroup::new_id(id.0),
                            ..default()
                        },
                    ));
                } else {
                    // on the client, just spawn the ball
                    // NOTE: the PreSpawnedPlayerObject component indicates that the entity will be spawned on both client and server
                    //  but the server will take authority as soon as the client receives the entity
                    commands.spawn((ball, PreSpawnedPlayerObject::default()));
                }
            }
        }
    }
}

pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<(&Transform, &ColorComponent), (Without<Confirmed>, With<PlayerId>)>,
    // // we will change the color of balls when they become predicted (i.e. adopt server authority)
    // prespawned_balls: Query<
    //     (&Transform, &ColorComponent),
    //     (
    //         With<PreSpawnedPlayerObject>,
    //         Without<Predicted>,
    //         With<BallMarker>,
    //     ),
    // >,
    // predicted_balls: Query<
    //     (&Transform, &ColorComponent),
    //     (
    //         Without<PreSpawnedPlayerObject>,
    //         With<Predicted>,
    //         With<BallMarker>,
    //     ),
    // >,
    balls: Query<(&Transform, &ColorComponent), (Without<Confirmed>, With<BallMarker>)>,
) {
    for (transform, color) in &players {
        // transform.rotation.angle_between()
        // let angle = transform.rotation.to_axis_angle().1;
        // warn!(axis = ?transform.rotation.to_axis_angle().0);
        gizmos.rect_2d(
            transform.translation.truncate(),
            // angle,
            transform.rotation.to_euler(EulerRot::XYZ).2,
            Vec2::ONE * PLAYER_SIZE,
            color.0,
        );
    }
    for (transform, color) in &balls {
        gizmos.circle_2d(transform.translation.truncate(), BALL_SIZE, color.0);
    }
    // for (transform, color) in &prespawned_balls {
    //     let color = color.0.set
    //     gizmos.circle_2d(transform.translation.truncate(), BALL_SIZE, color.0);
    // }
}
