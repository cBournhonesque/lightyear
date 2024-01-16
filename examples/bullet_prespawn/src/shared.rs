use crate::protocol::*;
use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy_screen_diagnostics::{Aggregate, ScreenDiagnostics, ScreenDiagnosticsPlugin};
use leafwing_input_manager::orientation::Orientation;
use leafwing_input_manager::prelude::ActionState;
use lightyear::client::prediction::{Rollback, RollbackState};
use lightyear::prelude::client::*;
use lightyear::prelude::TickManager;
use lightyear::prelude::*;
use lightyear::transport::io::IoDiagnosticsPlugin;
use std::time::Duration;
use tracing::Level;

const FRAME_HZ: f64 = 60.0;
const FIXED_TIMESTEP_HZ: f64 = 64.0;

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
            level: Level::WARN,
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
            // app.add_plugins(bevy_fps_counter::FpsCounterPlugin);

            // draw after interpolation is done
            app.add_systems(
                PostUpdate,
                draw_elements
                    .after(InterpolationSet::Interpolate)
                    .after(PredictionSet::VisualCorrection),
            );
            app.add_plugins(LogDiagnosticsPlugin {
                filter: Some(vec![
                    IoDiagnosticsPlugin::BYTES_IN,
                    IoDiagnosticsPlugin::BYTES_OUT,
                ]),
                ..default()
            });
            app.add_systems(Startup, setup_diagnostic);
            app.add_plugins(ScreenDiagnosticsPlugin::default());
        }

        // registry types for reflection
        app.register_type::<PlayerId>();
        app.add_systems(Update, (shoot_bullet, move_bullet));
    }
}

fn setup_diagnostic(mut onscreen: ResMut<ScreenDiagnostics>) {
    onscreen
        .add("bytes_in".to_string(), IoDiagnosticsPlugin::BYTES_IN)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:.0}"));
    onscreen
        .add("bytes_out".to_string(), IoDiagnosticsPlugin::BYTES_OUT)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:.0}"));
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: ClientId) -> Color {
    let h = ((client_id * 90) % 360) as f32;
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
    let mouse_position = action
        .action_data(PlayerActions::MoveCursor)
        .axis_pair
        .map(|axis| axis.xy())
        .unwrap_or_default();
    // warn!(?mouse_position);
    let angle =
        Vec2::new(0.0, 1.0).angle_between(mouse_position - transform.translation.truncate());
    transform.rotation = Quat::from_rotation_z(angle);
    // TODO: look_at should work
    // transform.look_at(Vec3::new(mouse_position.x, mouse_position.y, 0.0), Vec3::Y);
    if action.pressed(PlayerActions::Up) {
        transform.translation.y += PLAYER_MOVE_SPEED;
    }
    if action.pressed(PlayerActions::Down) {
        transform.translation.y -= PLAYER_MOVE_SPEED;
    }
    if action.pressed(PlayerActions::Right) {
        transform.translation.x += PLAYER_MOVE_SPEED;
    }
    if action.pressed(PlayerActions::Left) {
        transform.translation.x -= PLAYER_MOVE_SPEED;
    }
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn move_bullet(
    mut commands: Commands,
    mut query: Query<(Entity, &mut Transform), With<BallMarker>>,
) {
    const BALL_MOVE_SPEED: f32 = 20.0;
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
    mut query: Query<
        (&Transform, &ColorComponent, &mut ActionState<PlayerActions>),
        (Without<Interpolated>, Without<Confirmed>),
    >,
) {
    const BALL_MOVE_SPEED: f32 = 10.0;
    for (transform, color, mut action) in query.iter_mut() {
        // TODO: just_pressed should work
        if action.pressed(PlayerActions::Shoot) {
            action.consume(PlayerActions::Shoot);
            let ball = BallBundle::new(
                transform.translation.truncate(),
                transform.rotation.to_euler(EulerRot::XYZ).2,
                color.0,
                false,
            );
            commands.spawn(ball);
        }
    }
}

pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<(&Transform, &ColorComponent), (Without<Confirmed>, With<PlayerId>)>,
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
}
