use crate::protocol::*;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy_xpbd_2d::parry::shape::Ball;
use bevy_xpbd_2d::prelude::*;
use bevy_xpbd_2d::{PhysicsSchedule, PhysicsStepSet};
use leafwing_input_manager::prelude::ActionState;
use lightyear::client::prediction::{Rollback, RollbackState};
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use std::time::Duration;
use tracing::Level;

const FRAME_HZ: f64 = 60.0;
const FIXED_TIMESTEP_HZ: f64 = 64.0;
const MAX_VELOCITY: f32 = 200.0;

pub fn shared_config() -> SharedConfig {
    SharedConfig {
        enable_replication: true,
        client_send_interval: Duration::default(),
        server_send_interval: Duration::from_secs_f64(1.0 / 32.0),
        // server_send_interval: Duration::from_millis(100),
        tick: TickConfig {
            tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        },
        log: LogConfig {
            level: Level::INFO,
            filter: "wgpu=error,wgpu_hal=error,naga=warn,bevy_app=info,bevy_render=warn,quinn=warn"
                .to_string(),
        },
    }
}

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        if app.is_plugin_added::<RenderPlugin>() {
            // limit frame rate
            // app.add_plugins(bevy_framepace::FramepacePlugin);
            // app.world
            //     .resource_mut::<bevy_framepace::FramepaceSettings>()
            //     .limiter = bevy_framepace::Limiter::from_framerate(FRAME_HZ);

            // show framerate
            // use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
            // app.add_plugins(FrameTimeDiagnosticsPlugin::default());

            app.add_plugins(bevy_fps_counter::FpsCounterPlugin);

            // draw after interpolation is done
            app.add_systems(
                PostUpdate,
                draw_elements.after(InterpolationSet::Interpolate),
            );
        }

        // physics
        app.add_plugins(PhysicsPlugins::new(FixedUpdate))
            .insert_resource(Time::new_with(Physics::fixed_once_hz(FIXED_TIMESTEP_HZ)))
            .insert_resource(Gravity(Vec2::ZERO));
        app.configure_sets(
            FixedUpdate,
            // make sure that any physics simulation happens after the Main SystemSet
            // (where we apply user's actions)
            (
                PhysicsSet::Prepare,
                PhysicsSet::StepSimulation,
                PhysicsSet::Sync,
            )
                .in_set(FixedUpdateSet::Main),
        );
        // add a log at the start of the physics schedule
        app.add_systems(PhysicsSchedule, log.in_set(PhysicsStepSet::BroadPhase));

        if app.world.contains_resource::<Client>() {
            app.add_systems(
                FixedUpdate,
                after_physics_log::<Client>.after(FixedUpdateSet::Main),
            );
            // app.add_systems(Last, last_log::<Client>);
        }
        if app.world.contains_resource::<Server>() {
            app.add_systems(
                Last,
                after_physics_log::<Server>.after(FixedUpdateSet::Main),
            );
            // app.add_systems(Last, last_log::<Server>);
        }

        // registry types for reflection
        app.register_type::<PlayerId>();
    }
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: ClientId) -> Color {
    let h = ((client_id * 90) % 360) as f32;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(
    mut velocity: Mut<LinearVelocity>,
    action: &ActionState<PlayerActions>,
) {
    const MOVE_SPEED: f32 = 10.0;
    if action.pressed(PlayerActions::Up) {
        velocity.y += MOVE_SPEED;
    }
    if action.pressed(PlayerActions::Down) {
        velocity.y -= MOVE_SPEED;
    }
    if action.pressed(PlayerActions::Left) {
        velocity.x -= MOVE_SPEED;
    }
    if action.pressed(PlayerActions::Right) {
        velocity.x += MOVE_SPEED;
    }
    *velocity = LinearVelocity(velocity.clamp_length_max(MAX_VELOCITY));
}

pub(crate) fn after_physics_log<T: TickManaged>(
    ticker: Res<T>,
    rollback: Option<Res<Rollback>>,
    players: Query<(Entity, &Position), (Without<BallMarker>, Without<Confirmed>)>,
    ball: Query<&Position, (With<BallMarker>, Without<Confirmed>)>,
) {
    let mut tick = ticker.tick();
    if let Some(rollback) = rollback {
        if let RollbackState::ShouldRollback { current_tick } = rollback.state {
            tick = current_tick;
        }
    }
    for (entity, position) in players.iter() {
        info!(?tick, ?entity, ?position, "Player after physics update");
    }
    for position in ball.iter() {
        debug!(?tick, ?position, "Ball after physics update");
    }
}

pub(crate) fn last_log<T: TickManaged>(
    ticker: Res<T>,
    players: Query<(Entity, &Position), (Without<BallMarker>, Without<Confirmed>)>,
    ball: Query<&Position, (With<BallMarker>, Without<Confirmed>)>,
) {
    let tick = ticker.tick();
    for (entity, position) in players.iter() {
        info!(?tick, ?entity, ?position, "Player LAST update");
    }
    for position in ball.iter() {
        debug!(?tick, ?position, "Ball LAST update");
    }
}

pub(crate) fn log() {
    debug!("run physics schedule!");
}

/// System that draws the player's boxes and cursors
pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<
        (&Position, &Rotation, &ColorComponent),
        (Without<Confirmed>, Without<BallMarker>),
    >,
    balls: Query<(&Position, &ColorComponent), (Without<Confirmed>, With<BallMarker>)>,
    // players: Query<
    //     (&Position, &ColorComponent),
    //     (
    //         Without<Predicted>,
    //         Without<Interpolated>,
    //         Without<BallMarker>,
    //     ),
    // >,
    // cursors: Query<
    //     (&Position, &ColorComponent),
    //     (Without<Predicted>, Without<Interpolated>, With<BallMarker>),
    // >,
) {
    for (position, rotation, color) in &players {
        // gizmos.rect_2d(
        //     position.translation.truncate(),
        //     position.rotation.,
        //     Vec2::ONE * PLAYER_SIZE,
        //     color.0,
        // );
        gizmos.rect_2d(
            Vec2::new(position.x, position.y),
            rotation.as_radians(),
            Vec2::ONE * PLAYER_SIZE,
            color.0,
        );
    }
    for (position, color) in &balls {
        gizmos.circle_2d(Vec2::new(position.x, position.y), BALL_SIZE, color.0);
    }
}
