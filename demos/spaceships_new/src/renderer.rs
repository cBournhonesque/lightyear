use crate::entity_label::*;
/// Renders entities using gizmos to draw outlines
use crate::protocol::*;
use crate::shared::*;
use avian2d::parry::shape::SharedShape;
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::core_pipeline::bloom::Bloom;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use core::time::Duration;
use leafwing_input_manager::action_state::ActionState;
use lightyear::client::prediction::prespawn::PreSpawned;
use lightyear::inputs::leafwing::input_buffer::InputBuffer;
use lightyear::prelude::client::*;
use lightyear::prelude::Replicating;
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
use std::f32::consts::PI;
use std::f32::consts::TAU;

pub struct SpaceshipsRendererPlugin;

impl Plugin for SpaceshipsRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_camera);
        app.insert_resource(ClearColor::default());
        // app.insert_resource(ClearColor(css::DARK_GRAY.into()));
        let draw_shadows = false;
        // in an attempt to reduce flickering, draw walls before FixedUpdate runs
        // so they exist for longer during this tick.
        // retained gizmos may help us in bevy 0.15?
        app.add_systems(PreUpdate, draw_walls);

        // draw after visual interpolation has propagated
        app.add_systems(
            PostUpdate,
            (
                update_player_label,
                draw_confirmed_shadows.run_if(move || draw_shadows),
                draw_predicted_entities,
                draw_confirmed_entities.run_if(is_server),
                draw_explosions,
            )
                .chain()
                .after(bevy::transform::TransformSystem::TransformPropagate),
        );
        app.add_observer(add_player_label);

        app.add_systems(FixedPreUpdate, insert_bullet_mesh);

        app.add_plugins(EntityLabelPlugin);

        // set up visual interp plugins for Transform
        app.add_plugins(VisualInterpolationPlugin::<Transform>::default());

        // observers that add VisualInterpolationStatus components to entities which receive
        // a Position
        app.add_observer(add_visual_interpolation_components);
    }
}

// Non-wall entities get some visual interpolation by adding the lightyear
// VisualInterpolateStatus component
//
// We query filter With<Predicted> so that the correct client entities get visual-interpolation.
// We don't want to visually interpolate the client's Confirmed entities, since they are not rendered.
//
// We must trigger change detection so that the Transform updates from interpolation
// will be propagated to children (sprites, meshes, text, etc.)
fn add_visual_interpolation_components(
    // We use Position because it's added by avian later, and when it's added
    // we know that Predicted is already present on the entity
    trigger: Trigger<OnAdd, Position>,
    q: Query<Entity, (Without<Wall>, With<Predicted>)>,
    mut commands: Commands,
) {
    if !q.contains(trigger.target()) {
        return;
    }
    debug!("Adding visual interp component to {:?}", trigger.target());
    commands
        .entity(trigger.target())
        .insert(VisualInterpolateStatus::<Transform> {
            // We must trigger change detection on visual interpolation
            // to make sure that child entities (sprites, meshes, text)
            // are also interpolated
            trigger_change_detection: true,
            ..default()
        });
}

fn init_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Camera {
            hdr: true,
            ..default()
        },
        Tonemapping::TonyMcMapface,
        Bloom::default(),
        Visibility::default(),
    ));
}

fn add_player_label(
    trigger: Trigger<OnAdd, Player>,
    mut commands: Commands,
    // add the label on both client and server
    q: Query<(Entity, &Player, &Score), Or<(With<Predicted>, With<Replicating>)>>,
) {
    if let Ok((e, player, score)) = q.get(trigger.entity()) {
        error!("Adding visual bits to {e:?}");
        commands.entity(e).insert((
            Visibility::default(),
            Transform::default(),
            EntityLabel {
                text: format!("{} <{}>\n", player.nickname, score.0),
                color: css::ANTIQUE_WHITE.with_alpha(0.8).into(),
                offset: Vec2::Y * -45.0,
                ..Default::default()
            },
        ));
    }
}

// update the labels when the player rtt/jitter is updated by the server
fn update_player_label(
    mut q: Query<
        (
            Entity,
            &Player,
            &mut EntityLabel,
            &InputBuffer<PlayerActions>,
            &Score,
        ),
        Or<(
            Changed<Player>,
            Changed<Score>,
            Changed<InputBuffer<PlayerActions>>,
        )>,
    >,
    tick_manager: Res<TickManager>,
) {
    for (e, player, mut label, input_buffer, score) in q.iter_mut() {
        // hopefully this is positive, ie we have received remote player inputs before they are needed.
        // this can happen because of input_delay. The server receives inputs in advance of
        // needing them, and rebroadcasts to other players.
        let num_buffered_inputs = if let Some(end_tick) = input_buffer.end_tick() {
            lightyear::utils::wrapping_id::wrapping_diff(tick_manager.tick().0, end_tick.0)
        } else {
            0
        };
        label.text = format!("{} <{}>\n", player.nickname, score.0);
        label.sub_text = format!(
            "{}~{}ms [{num_buffered_inputs}]",
            player.rtt.as_millis(),
            player.jitter.as_millis()
        );
    }
}

/// System that draws the outlines of confirmed entities, with lines to the centre of their predicted location.
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
        let ghost_col = color.0.with_alpha(0.2 + speed * 0.8);
        render_shape(collider.shape(), position, rotation, &mut gizmos, ghost_col);
        gizmos.line_2d(**position, **pred_pos, ghost_col);
    }
}

/// System that draws the player's boxes and cursors
fn draw_predicted_entities(
    mut gizmos: Gizmos,
    predicted: Query<
        (
            Entity,
            &Position,
            &Rotation,
            &ColorComponent,
            &Collider,
            Has<PreSpawned>,
            Option<&ActionState<PlayerActions>>,
            Option<&InputBuffer<PlayerActions>>,
        ),
        (
            // skip drawing bullet outlines, since we add a mesh + material to them
            Without<BulletMarker>,
            Or<(With<PreSpawned>, With<Predicted>)>,
        ),
    >,
    tick_manager: Res<TickManager>,
) {
    for (e, position, rotation, color, collider, prespawned, opt_action, opt_ib) in &predicted {
        // render prespawned translucent until acknowledged by the server
        // (at which point the PreSpawned component is removed)
        let col = if prespawned {
            color.0.with_alpha(0.5)
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
            // draw an engine exhaust triangle
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
                (col.to_linear() * 2.5).into(), // bloom
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
                    col.0.with_alpha(0.7),
                );
            }
        }
    }
}

// draws explosion effects, and despawns them once they expire
fn draw_explosions(
    mut gizmos: Gizmos,
    q: Query<(Entity, &Explosion, &Transform)>,
    time: Res<Time>,
    mut commands: Commands,
) {
    for (e, explosion, transform) in q.iter() {
        if let Some((color, radius)) = explosion.compute_at_time(time.elapsed()) {
            gizmos.circle_2d(transform.translation.xy(), radius, color);
        } else {
            commands.entity(e).despawn();
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
        let p1 = pos.0 + rot * Vec2::new(triangle.a[0], triangle.a[1]);
        let p2 = pos.0 + rot * Vec2::new(triangle.b[0], triangle.b[1]);
        let p3 = pos.0 + rot * Vec2::new(triangle.c[0], triangle.c[1]);
        gizmos.line_2d(p1, p2, render_color);
        gizmos.line_2d(p2, p3, render_color);
        gizmos.line_2d(p3, p1, render_color);
    } else if let Some(poly) = shape.as_convex_polygon() {
        let last_p = poly.points().last().unwrap();
        let mut start_p = pos.0 + (rot * Vec2::new(last_p.x, last_p.y));
        for i in 0..poly.points().len() {
            let p = poly.points()[i];
            let tmp = pos.0 + (rot * Vec2::new(p.x, p.y));
            gizmos.line_2d(start_p, tmp, render_color);
            start_p = tmp;
        }
    } else if let Some(cuboid) = shape.as_cuboid() {
        let points: Vec<Vec3> = cuboid
            .to_polyline()
            .into_iter()
            .map(|p| Vec3::new(p.x, p.y, 0.0))
            .collect();
        let mut start_p = pos.0 + (rot * points.last().unwrap().truncate());
        for point in &points {
            let tmp = pos.0 + (rot * point.truncate());
            gizmos.line_2d(start_p, tmp, render_color);
            start_p = tmp;
        }
    } else if let Some(ball) = shape.as_ball() {
        gizmos.circle_2d(pos.0, ball.radius, render_color);
    } else {
        panic!("unimplemented render");
    }
}

pub fn insert_bullet_mesh(
    q: Query<(Entity, &Collider, &ColorComponent), (With<BulletMarker>, Added<Collider>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    for (entity, collider, col) in q.iter() {
        let ball = collider
            .shape()
            .as_ball()
            .expect("Bullets expected to be balls.");
        let ball = Circle::new(ball.radius);
        let mesh = Mesh::from(ball);
        commands.entity(entity).insert((
            Mesh2d(meshes.add(mesh)),
            Transform::from_translation(Vec3::Z),
            MeshMaterial2d(materials.add(ColorMaterial::from(col.0))),
        ));
    }
}

#[derive(Component)]
pub struct Explosion {
    spawn_time: Duration,
    max_age: Duration,
    color: Color,
    initial_radius: f32,
}

impl Explosion {
    pub fn new(now: Duration, color: Color) -> Self {
        Self {
            spawn_time: now,
            max_age: Duration::from_millis(70),
            initial_radius: BULLET_SIZE,
            color,
        }
    }

    // Gives a color and radius based on elapsed time, for a simple visual explosion effect.
    //
    // None = despawn due to expiry.
    pub fn compute_at_time(&self, now: Duration) -> Option<(Color, f32)> {
        let age = now - self.spawn_time;
        if age > self.max_age {
            return None;
        }
        // starts at 0.0, once max_age reached, is 1.0.
        let progress = (age.as_secs_f32() / self.max_age.as_secs_f32()).clamp(0.0, 1.0);
        let color = self.color.with_alpha(1.0 - progress);
        let radius = self.initial_radius + (1.0 - progress) * self.initial_radius * 3.0;
        Some((color, radius))
    }
}
