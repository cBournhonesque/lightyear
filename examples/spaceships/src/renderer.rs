use std::f32::consts::PI;
use std::f32::consts::TAU;
use std::time::Duration;

use crate::entity_label::*;
/// Renders entities using gizmos to draw outlines
use crate::protocol::*;
use crate::shared::*;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use bevy_screen_diagnostics::ScreenEntityDiagnosticsPlugin;
use bevy_screen_diagnostics::ScreenFrameDiagnosticsPlugin;
use bevy_screen_diagnostics::{Aggregate, ScreenDiagnostics, ScreenDiagnosticsPlugin};
use bevy_xpbd_2d::parry::shape::{Ball, SharedShape};
use bevy_xpbd_2d::prelude::*;
use bevy_xpbd_2d::{PhysicsSchedule, PhysicsStepSet};
use leafwing_input_manager::action_state::ActionState;
use lightyear::client::prediction::prespawn::PreSpawnedPlayerObject;
use lightyear::inputs::leafwing::input_buffer::InputBuffer;
use lightyear::prelude::client::*;
use lightyear::shared::tick_manager;
use lightyear::shared::tick_manager::Tick;
use lightyear::shared::tick_manager::TickManager;
use lightyear::transport::io::IoDiagnosticsPlugin;
use lightyear::{
    client::{
        interpolation::plugin::InterpolationSet,
        prediction::{diagnostics::PredictionDiagnosticsPlugin, plugin::PredictionSet},
    },
    shared::run_conditions::is_server,
};

pub struct SpaceshipsRendererPlugin;

impl Plugin for SpaceshipsRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_camera);
        app.insert_resource(ClearColor(Color::DARK_GRAY));
        let draw_shadows = false;
        // draw last to ensure all the interpolation/synching stuff has happened
        app.add_systems(
            Last,
            (
                add_visual_components,
                update_visual_components,
                draw_walls,
                draw_confirmed_shadows.run_if(move || draw_shadows),
                draw_predicted_entities,
                draw_confirmed_entities.run_if(is_server),
            )
                .chain(),
        );

        app.add_systems(Startup, setup_diagnostic);
        app.add_plugins(ScreenDiagnosticsPlugin::default());
        app.add_plugins(ScreenEntityDiagnosticsPlugin);
        // app.add_plugins(ScreenFrameDiagnosticsPlugin);
        app.add_plugins(EntityLabelPlugin {
            config: EntityLabelConfig {
                font: "fonts/quicksand-light.ttf".to_owned(),
            },
        });
        // probably want to avoid using this on server, if server gui enabled
        app.add_plugins(VisualInterpolationPlugin::<Position>::default());
        app.add_plugins(VisualInterpolationPlugin::<Rotation>::default());
    }
}

fn init_camera(mut commands: Commands, mut windows: Query<&mut Window>) {
    let mut window = windows.single_mut();
    window.resolution.set(800., 800.);
    commands.spawn(Camera2dBundle::default());
}

// add visual interp components on client predicted entities
fn add_visual_components(
    mut commands: Commands,
    q: Query<
        (Entity, &Player),
        (
            With<Predicted>,
            Added<Collider>,
            Without<VisualInterpolateStatus<Position>>,
            Without<VisualInterpolateStatus<Rotation>>,
        ),
    >,
) {
    for (e, player) in q.iter() {
        // info!("Adding visual bits to {e:?}");
        commands.entity(e).insert((
            VisibilityBundle::default(),
            TransformBundle::default(),
            EntityLabel {
                text: player.nickname.clone(),
                color: Color::ANTIQUE_WHITE.with_a(0.8),
                offset: Vec2::Y * -45.0,
                ..Default::default()
            },
            VisualInterpolateStatus::<Position>::default(),
            VisualInterpolateStatus::<Rotation>::default(),
        ));
    }
}

// update the labels when the player rtt/jitter is updated by the server
fn update_visual_components(
    mut q: Query<
        (
            Entity,
            &Player,
            &mut EntityLabel,
            &InputBuffer<PlayerActions>,
        ),
        Changed<Player>,
    >,
    tick_manager: Res<TickManager>,
) {
    for (e, player, mut label, input_buffer) in q.iter_mut() {
        // hopefully this is +ve, ie we have received remote player inputs before they are needed.
        // this can happen because of input_delay. The server receives inputs in advance of
        // needing them, and rebroadcasts to other players.
        let num_buffered_inputs = if let Some(end_tick) = input_buffer.end_tick() {
            lightyear::utils::wrapping_id::wrapping_diff(tick_manager.tick().0, end_tick.0)
        } else {
            0
        };
        label.sub_text = format!(
            "{}±{}ms [{num_buffered_inputs}]",
            player.rtt.as_millis(),
            player.jitter.as_millis()
        );
    }
}

fn setup_diagnostic(mut onscreen: ResMut<ScreenDiagnostics>) {
    onscreen
        .add("RB".to_string(), PredictionDiagnosticsPlugin::ROLLBACKS)
        .aggregate(Aggregate::Value)
        .format(|v| format!("{v:.0}"));
    onscreen
        .add(
            "RBt".to_string(),
            PredictionDiagnosticsPlugin::ROLLBACK_TICKS,
        )
        .aggregate(Aggregate::Value)
        .format(|v| format!("{v:.0}"));
    onscreen
        .add(
            "RBd".to_string(),
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

/// System that draws the outlines of confirmed entities, with lines to the centre of their predicted location.
#[allow(clippy::type_complexity)]
pub(crate) fn draw_confirmed_shadows(
    mut gizmos: Gizmos,
    confirmed_q: Query<
        (&Position, &Rotation, &LinearVelocity, &Confirmed),
        Or<(With<Player>, With<BallMarker>, With<BulletMarker>)>,
    >,
    predicted_q: Query<
        (&Position, &Collider, &ColorComponent),
        (
            With<Predicted>,
            Or<(With<Player>, With<BallMarker>, With<BulletMarker>)>,
        ),
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
    predicted: Query<
        (
            Entity,
            &Position,
            &Rotation,
            &ColorComponent,
            &Collider,
            Has<PreSpawnedPlayerObject>,
            Option<&ActionState<PlayerActions>>,
            Option<&InputBuffer<PlayerActions>>,
        ),
        Or<(With<PreSpawnedPlayerObject>, With<Predicted>)>,
    >,
    tick_manager: Res<TickManager>,
) {
    for (e, position, rotation, color, collider, prespawned, opt_action, opt_ib) in &predicted {
        // render prespawned translucent until acknowledged by the server
        // (at which point the PreSpawnedPlayerObject component is removed)
        let col = if prespawned {
            color.0.with_a(0.5)
        } else {
            color.0
        };

        render_shape(collider.shape(), position, rotation, &mut gizmos, col);
        // render engine exhaust for players holding down thrust.
        let Some(action) = opt_action else {
            continue;
        };
        let Some(ib) = opt_ib else {
            continue;
        };
        let mut is_thrusting = action.pressed(&PlayerActions::Up);
        if !is_thrusting {
            // if inputs are late for this player, we'll render the engine if their
            // last input was thrust. otherwise remote players with late inputs will never
            // appear to be thrusting, since it all happens in rollback.
            if let Some(action) = ib.get_last() {
                is_thrusting = action.pressed(&PlayerActions::Up);
            }
        }

        if is_thrusting {
            let width = 0.6 * (SHIP_WIDTH / 2.0);
            let points = vec![
                Vec2::new(width, (-SHIP_LENGTH / 2.) - 3.0),
                Vec2::new(-width, (-SHIP_LENGTH / 2.) - 3.0),
                Vec2::new(0.0, (-SHIP_LENGTH / 2.) - 10.0),
            ];
            let collider = Collider::convex_hull(points).unwrap();
            render_shape(
                collider.shape(),
                position,
                rotation,
                &mut gizmos,
                col.with_a(0.7),
            );
        }
    }
}

fn draw_walls(walls: Query<&Wall, Without<Player>>, mut gizmos: Gizmos) {
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
        (
            &Position,
            &Rotation,
            &ColorComponent,
            &Collider,
            Option<&ActionState<PlayerActions>>,
            &ColorComponent,
        ),
        Or<(With<Player>, With<BallMarker>, With<BulletMarker>)>,
    >,
) {
    for (position, rotation, color, collider, opt_action, col) in &confirmed {
        render_shape(collider.shape(), position, rotation, &mut gizmos, color.0);
        // render engine exhaust for players holding down thrust.
        if let Some(action) = opt_action {
            if action.pressed(&PlayerActions::Up) {
                let width = 0.6 * (SHIP_WIDTH / 2.0);
                let points = vec![
                    Vec2::new(width, (-SHIP_LENGTH / 2.) - 3.0),
                    Vec2::new(-width, (-SHIP_LENGTH / 2.) - 3.0),
                    Vec2::new(0.0, (-SHIP_LENGTH / 2.) - 10.0),
                ];
                let collider = Collider::convex_hull(points).unwrap();
                render_shape(
                    collider.shape(),
                    position,
                    rotation,
                    &mut gizmos,
                    col.0.with_a(0.7),
                );
            }
        }
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
