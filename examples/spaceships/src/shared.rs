use bevy::diagnostic::LogDiagnosticsPlugin;
use bevy::ecs::query::QueryData;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::utils::Duration;
use bevy_screen_diagnostics::{Aggregate, ScreenDiagnostics, ScreenDiagnosticsPlugin};
use bevy_xpbd_2d::parry::shape::{Ball, SharedShape};
use bevy_xpbd_2d::prelude::*;
use bevy_xpbd_2d::{PhysicsSchedule, PhysicsStepSet};
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
            let draw_shadows = self.show_confirmed;
            // draw after interpolation is done
            app.add_systems(
                PostUpdate,
                (
                    draw_walls,
                    draw_confirmed_shadows.run_if(move || draw_shadows),
                    draw_predicted_entities,
                    draw_confirmed_entities.run_if(is_server),
                )
                    .chain()
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
        // bundles
        app.add_systems(Startup, init);

        // physics
        app.add_plugins(PhysicsPlugins::new(FixedUpdate))
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
        app.add_systems(PhysicsSchedule, log.in_set(PhysicsStepSet::BroadPhase));

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

#[derive(QueryData)]
#[query_data(mutable, derive(Debug))]
pub struct ApplyInputsQuery {
    pub ex_force: &'static mut ExternalForce,
    pub ang_vel: &'static mut AngularVelocity,
    pub rot: &'static Rotation,
    pub action: &'static ActionState<PlayerActions>,
}

pub fn shared_movement_behaviour(aiq: ApplyInputsQueryItem) {
    const THRUSTER_POWER: f32 = 32000.;
    const ROTATIONAL_SPEED: f32 = 4.0;
    let ApplyInputsQueryItem {
        mut ex_force,
        mut ang_vel,
        rot,
        action,
    } = aiq;

    // info!("pressed: {:?}", action.get_pressed());

    if action.pressed(&PlayerActions::Up) {
        ex_force
            .apply_force(rot.rotate(Vec2::Y * THRUSTER_POWER))
            .with_persistence(false);
    }
    let desired_ang_vel = if action.pressed(&PlayerActions::Left) {
        ROTATIONAL_SPEED
    } else if action.pressed(&PlayerActions::Right) {
        -ROTATIONAL_SPEED
    } else {
        0.0
    };
    if ang_vel.0 != desired_ang_vel {
        ang_vel.0 = desired_ang_vel;
    }
}

// // This system defines how we update the player's positions when we receive an input
// pub(crate) fn shared_movement_behaviour(
//     mut velocity: Mut<LinearVelocity>,
//     action: &ActionState<PlayerActions>,
// ) {
//     const MOVE_SPEED: f32 = 10.0;
//     if action.pressed(&PlayerActions::Up) {
//         velocity.y += MOVE_SPEED;
//     }
//     if action.pressed(&PlayerActions::Down) {
//         velocity.y -= MOVE_SPEED;
//     }
//     if action.pressed(&PlayerActions::Left) {
//         velocity.x -= MOVE_SPEED;
//     }
//     if action.pressed(&PlayerActions::Right) {
//         velocity.x += MOVE_SPEED;
//     }
//     *velocity = LinearVelocity(velocity.clamp_length_max(MAX_VELOCITY));
// }

pub(crate) fn after_physics_log(
    tick_manager: Res<TickManager>,
    rollback: Option<Res<Rollback>>,
    players: Query<
        (Entity, &Position, &Rotation),
        (Without<BallMarker>, Without<Confirmed>, With<PlayerId>),
    >,
    ball: Query<&Position, (With<BallMarker>, Without<Confirmed>)>,
) {
    let tick = rollback.map_or(tick_manager.tick(), |r| {
        tick_manager.tick_or_rollback_tick(r.as_ref())
    });
    for (entity, position, rotation) in players.iter() {
        debug!(
            ?tick,
            ?entity,
            ?position,
            rotation = ?rotation.as_degrees(),
            "Player after physics update"
        );
    }
    for position in ball.iter() {
        debug!(?tick, ?position, "Ball after physics update");
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
    ball: Query<&Position, (With<BallMarker>, Without<Confirmed>)>,
) {
    let tick = tick_manager.tick();
    for (entity, position, rotation, correction, rotation_correction) in players.iter() {
        debug!(?tick, ?entity, ?position, ?correction, "Player LAST update");
        debug!(
            ?tick,
            ?entity,
            rotation = ?rotation.as_degrees(),
            ?rotation_correction,
            "Player LAST update"
        );
    }
    for position in ball.iter() {
        debug!(?tick, ?position, "Ball LAST update");
    }
}

pub(crate) fn log() {
    debug!("run physics schedule!");
}

/// System that draws the outlines of confirmed entities, with lines to the centre of their predicted location.
#[allow(clippy::type_complexity)]
pub(crate) fn draw_confirmed_shadows(
    mut gizmos: Gizmos,
    confirmed_q: Query<
        (&Position, &Rotation, &LinearVelocity, &Confirmed),
        Or<(With<PlayerId>, With<BallMarker>)>,
    >,
    predicted_q: Query<
        (&Position, &Collider, &ColorComponent),
        (With<Predicted>, Or<(With<PlayerId>, With<BallMarker>)>),
    >,
) {
    for (position, rotation, velocity, confirmed) in confirmed_q.iter() {
        let Some(pred_entity) = confirmed.predicted else {
            continue;
        };
        let Ok((pred_pos, collider, color)) = predicted_q.get(pred_entity) else {
            continue;
        };
        let speed = velocity.length() / MAX_VELOCITY;
        let ghost_col = color.0.with_a(0.2 + speed * 0.8);
        render_shape(collider.shape(), position, rotation, &mut gizmos, ghost_col);
        gizmos.line_2d(**position, **pred_pos, ghost_col);
    }
}

/// System that draws the player's boxes and cursors
#[allow(clippy::type_complexity)]
fn draw_predicted_entities(
    mut gizmos: Gizmos,
    players: Query<
        (&Position, &Rotation, &ColorComponent, &Collider),
        (With<Predicted>, With<PlayerId>),
    >,
    balls: Query<
        (&Position, &Rotation, &ColorComponent, &Collider),
        (With<Predicted>, With<BallMarker>),
    >,
) {
    for (position, rotation, color, collider) in &players {
        render_shape(collider.shape(), position, rotation, &mut gizmos, color.0);
    }
    for (position, rotation, color, collider) in &balls {
        render_shape(collider.shape(), position, rotation, &mut gizmos, color.0);
    }
}

fn draw_walls(walls: Query<&Wall, Without<PlayerId>>, mut gizmos: Gizmos) {
    for wall in &walls {
        gizmos.line_2d(wall.start, wall.end, Color::WHITE);
    }
}

/// Draws confirmed entities that have colliders.
/// Only useful on the server
#[allow(clippy::type_complexity)]
fn draw_confirmed_entities(
    mut gizmos: Gizmos,
    confirmed: Query<
        (&Position, &Rotation, &ColorComponent, &Collider),
        Or<(With<PlayerId>, With<BallMarker>)>,
    >,
) {
    for (position, rotation, color, collider) in &confirmed {
        render_shape(collider.shape(), position, rotation, &mut gizmos, color.0);
    }
}

/// renders various shapes using gizmos
pub fn render_shape(
    shape: &SharedShape,
    pos: &Position,
    rot: &Rotation,
    gizmos: &mut Gizmos,
    render_color: Color,
) {
    if let Some(triangle) = shape.as_triangle() {
        let p1 = pos.0 + rot.rotate(Vec2::new(triangle.a[0], triangle.a[1]));
        let p2 = pos.0 + rot.rotate(Vec2::new(triangle.b[0], triangle.b[1]));
        let p3 = pos.0 + rot.rotate(Vec2::new(triangle.c[0], triangle.c[1]));
        gizmos.line_2d(p1, p2, render_color);
        gizmos.line_2d(p2, p3, render_color);
        gizmos.line_2d(p3, p1, render_color);
    } else if let Some(poly) = shape.as_convex_polygon() {
        let last_p = poly.points().last().unwrap();
        let mut start_p = pos.0 + rot.rotate(Vec2::new(last_p.x, last_p.y));
        for i in 0..poly.points().len() {
            let p = poly.points()[i];
            let tmp = pos.0 + rot.rotate(Vec2::new(p.x, p.y));
            gizmos.line_2d(start_p, tmp, render_color);
            start_p = tmp;
        }
    } else if let Some(cuboid) = shape.as_cuboid() {
        let points: Vec<Vec3> = cuboid
            .to_polyline()
            .into_iter()
            .map(|p| Vec3::new(p.x, p.y, 0.0))
            .collect();
        let mut start_p = pos.0 + rot.rotate(points.last().unwrap().truncate());
        for point in &points {
            let tmp = pos.0 + rot.rotate(point.truncate());
            gizmos.line_2d(start_p, tmp, render_color);
            start_p = tmp;
        }
    } else if let Some(ball) = shape.as_ball() {
        gizmos.circle_2d(pos.0, ball.radius, render_color);
    } else {
        panic!("unimplented render");
    }
}

// Wall
#[derive(Bundle)]
pub(crate) struct WallBundle {
    color: ColorComponent,
    physics: PhysicsBundle,
    wall: Wall,
    name: Name,
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
                external_force: ExternalForce::default(),
            },
            wall: Wall { start, end },
            name: Name::new("Wall"),
        }
    }
}
