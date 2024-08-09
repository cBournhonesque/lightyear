use avian2d::dynamics::solver::SolverConfig;
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::utils::Duration;
use bevy_screen_diagnostics::{Aggregate, ScreenDiagnostics, ScreenDiagnosticsPlugin};
use leafwing_input_manager::prelude::ActionState;
use tracing::Level;

use lightyear::client::prediction::diagnostics::PredictionDiagnosticsPlugin;
use lightyear::prelude::client::*;
use lightyear::prelude::TickManager;
use lightyear::prelude::*;
use lightyear::shared::ping::diagnostics::PingDiagnosticsPlugin;
use lightyear::transport::io::IoDiagnosticsPlugin;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

use crate::protocol::*;
const MAX_VELOCITY: f32 = 200.0;
const WALL_SIZE: f32 = 350.0;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum FixedSet {
    // main fixed update systems (handle inputs)
    Main,
    // apply physics steps
    Physics,
}

#[derive(Clone)]
pub struct SharedPlugin {
    pub(crate) show_confirmed: bool,
}

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        if app.is_plugin_added::<RenderPlugin>() {
            app.add_systems(Startup, init_camera);

            // draw after interpolation is done
            app.add_systems(
                PostUpdate,
                draw_elements
                    .after(InterpolationSet::Interpolate)
                    .after(PredictionSet::VisualCorrection),
            );
            if self.show_confirmed {
                app.add_systems(
                    PostUpdate,
                    draw_confirmed_shadows
                        .after(InterpolationSet::Interpolate)
                        .after(PredictionSet::VisualCorrection),
                );
            }
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
        // bundles
        app.add_systems(Startup, init);

        // physics
        app.add_plugins(
            PhysicsPlugins::new(FixedUpdate)
                .build()
                .disable::<SleepingPlugin>()
                .disable::<ColliderHierarchyPlugin>(),
        )
        .insert_resource(SubstepCount(12))
        .insert_resource(SolverConfig {
            warm_start_coefficient: 0.0,
            ..default()
        })
        .insert_resource(Time::new_with(Physics::fixed_once_hz(FIXED_TIMESTEP_HZ)))
        .insert_resource(Gravity(Vec2::ZERO));
        app.configure_sets(
            FixedUpdate,
            (
                // make sure that any physics simulation happens after the Main SystemSet
                // (where we apply user's actions)
                (
                    PhysicsSet::Prepare,
                    PhysicsSet::StepSimulation,
                    PhysicsSet::Sync,
                )
                    .in_set(FixedSet::Physics),
                (FixedSet::Main, FixedSet::Physics).chain(),
            ),
        );
        // add a log at the start of the physics schedule
        app.add_systems(PhysicsSchedule, log.in_set(PhysicsStepSet::First));

        app.add_systems(FixedPostUpdate, after_physics_log);
        app.add_systems(Last, last_log);

        // registry types for reflection
        app.register_type::<PlayerId>();
    }
}

fn setup_diagnostic(mut onscreen: ResMut<ScreenDiagnostics>) {
    onscreen
        .add(
            "Rollbacks".to_string(),
            PredictionDiagnosticsPlugin::ROLLBACKS,
        )
        .aggregate(Aggregate::Value)
        .format(|v| format!("{v:.0}"));
    onscreen
        .add(
            "Rollback ticks".to_string(),
            PredictionDiagnosticsPlugin::ROLLBACK_TICKS,
        )
        .aggregate(Aggregate::Value)
        .format(|v| format!("{v:.0}"));
    onscreen
        .add(
            "RB depth".to_string(),
            PredictionDiagnosticsPlugin::ROLLBACK_DEPTH,
        )
        .aggregate(Aggregate::Value)
        .format(|v| format!("{v:.1}"));
    // screen diagnostics twitches due to layout change when a metric adds or removes
    // a digit, so pad these metrics to 3 digits.
    onscreen
        .add("KB_in".to_string(), IoDiagnosticsPlugin::BYTES_IN)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:0>3.0}"));
    onscreen
        .add("KB_out".to_string(), IoDiagnosticsPlugin::BYTES_OUT)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:0>3.0}"));
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: ClientId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

fn init_camera(mut commands: Commands) {
    commands.spawn(Camera2dBundle::default());
}

pub(crate) fn init(mut commands: Commands) {
    commands.spawn(WallBundle::new(
        Vec2::new(-WALL_SIZE, -WALL_SIZE),
        Vec2::new(-WALL_SIZE, WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(-WALL_SIZE, WALL_SIZE),
        Vec2::new(WALL_SIZE, WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(WALL_SIZE, WALL_SIZE),
        Vec2::new(WALL_SIZE, -WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(WALL_SIZE, -WALL_SIZE),
        Vec2::new(-WALL_SIZE, -WALL_SIZE),
        Color::WHITE,
    ));
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(
    mut velocity: Mut<LinearVelocity>,
    action: &ActionState<PlayerActions>,
) {
    const MOVE_SPEED: f32 = 10.0;
    if action.pressed(&PlayerActions::Up) {
        velocity.y += MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Down) {
        velocity.y -= MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Left) {
        velocity.x -= MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Right) {
        velocity.x += MOVE_SPEED;
    }
    *velocity = LinearVelocity(velocity.clamp_length_max(MAX_VELOCITY));
}

pub(crate) fn after_physics_log(
    tick_manager: Res<TickManager>,
    rollback: Option<Res<Rollback>>,
    players: Query<
        (Entity, &Position, &Rotation),
        (Without<BallMarker>, Without<Confirmed>, With<PlayerId>),
    >,
    ball: Query<(&Position, &Rotation), (With<BallMarker>, Without<Confirmed>)>,
) {
    let tick = rollback.map_or(tick_manager.tick(), |r| {
        tick_manager.tick_or_rollback_tick(r.as_ref())
    });
    for (entity, position, rotation) in players.iter() {
        trace!(
            ?tick,
            ?entity,
            ?position,
            rotation = ?rotation.as_degrees(),
            "Player after physics update"
        );
    }
    for (position, rotation) in ball.iter() {
        trace!(?tick, ?position, ?rotation, "Ball after physics update");
    }
}

pub(crate) fn last_log(
    tick_manager: Res<TickManager>,
    players: Query<
        (
            Entity,
            &Position,
            &Rotation,
            Option<&Correction<Position>>,
            Option<&Correction<Rotation>>,
        ),
        (Without<BallMarker>, Without<Confirmed>, With<PlayerId>),
    >,
    ball: Query<(&Position, &Rotation), (With<BallMarker>, Without<Confirmed>)>,
) {
    let tick = tick_manager.tick();
    for (entity, position, rotation, correction, rotation_correction) in players.iter() {
        trace!(?tick, ?entity, ?position, ?correction, "Player LAST update");
        trace!(
            ?tick,
            ?entity,
            rotation = ?rotation.as_degrees(),
            ?rotation_correction,
            "Player LAST update"
        );
    }
    for (position, rotation) in ball.iter() {
        trace!(?tick, ?position, ?rotation, "Ball LAST update");
    }
}

pub(crate) fn log() {
    trace!("run physics schedule!");
}

/// System that draws the outlines of confirmed entities, with lines to the centre of their predicted location.
pub(crate) fn draw_confirmed_shadows(
    mut gizmos: Gizmos,
    confirmed_q: Query<(&Position, &Rotation, &LinearVelocity, &Confirmed), With<PlayerId>>,
    predicted_q: Query<&Position, With<PlayerId>>,
) {
    for (position, rotation, velocity, confirmed) in confirmed_q.iter() {
        let speed = velocity.length() / MAX_VELOCITY;
        let ghost_col = css::GRAY.with_alpha(speed);
        gizmos.rect_2d(
            Vec2::new(position.x, position.y),
            rotation.as_radians(),
            Vec2::ONE * PLAYER_SIZE,
            ghost_col,
        );
        if let Some(e) = confirmed.predicted {
            if let Ok(pos) = predicted_q.get(e) {
                gizmos.line_2d(**position, **pos, ghost_col);
            }
        }
    }
}

/// System that draws the player's boxes and cursors
pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<(&Position, &Rotation, &ColorComponent), (Without<Confirmed>, With<PlayerId>)>,
    balls: Query<(&Position, &ColorComponent), (Without<Confirmed>, With<BallMarker>)>,
    walls: Query<(&Wall, &ColorComponent), (Without<BallMarker>, Without<PlayerId>)>,
) {
    for (position, rotation, color) in &players {
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
    for (wall, color) in &walls {
        gizmos.line_2d(wall.start, wall.end, color.0);
    }
}

// Wall
#[derive(Bundle)]
pub(crate) struct WallBundle {
    color: ColorComponent,
    physics: PhysicsBundle,
    wall: Wall,
}

#[derive(Component)]
pub(crate) struct Wall {
    start: Vec2,
    end: Vec2,
}

impl WallBundle {
    pub(crate) fn new(start: Vec2, end: Vec2, color: Color) -> Self {
        Self {
            color: ColorComponent(color),
            physics: PhysicsBundle {
                collider: Collider::segment(start, end),
                collider_density: ColliderDensity(1.0),
                rigid_body: RigidBody::Static,
            },
            wall: Wall { start, end },
        }
    }
}
