use crate::protocol::*;
use crate::shared::BOT_RADIUS;
use avian2d::prelude::*;
use bevy::color::palettes::basic::GREEN;
use bevy::color::palettes::css::BLUE;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use lightyear::connection::host::HostServer;
use lightyear::prelude::{
    lightyear_debug_event, Client, DebugCategory, DebugSamplePoint, InputTimeline, Interpolated,
    IsSynced, LocalTimeline, PreSpawned, Predicted, Replicate, Replicated, Server,
};
use lightyear_avian2d::prelude::AabbEnvelopeHolder;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);

        app.add_observer(add_interpolated_bot_visuals);
        app.add_observer(add_predicted_bot_visuals);
        app.add_observer(add_bullet_visuals);
        app.add_observer(add_player_visuals);

        #[cfg(feature = "client")]
        app.add_systems(
            FixedPostUpdate,
            hide_interpolated_bullets_after_local_hit.after(PhysicsSystems::StepSimulation),
        );
        app.add_plugins(FrameInterpolationPlugin::<Transform>::default());

        #[cfg(feature = "client")]
        {
            app.add_systems(Update, display_score);
        }

        #[cfg(feature = "server")]
        {
            app.add_systems(PostUpdate, draw_aabb_envelope);
        }
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
    #[cfg(feature = "client")]
    commands.spawn((
        Text::new("Score: 0"),
        TextFont::from_font_size(30.0),
        TextColor(Color::WHITE.with_alpha(0.5)),
        Node {
            align_self: AlignSelf::End,
            ..default()
        },
        ScoreText,
    ));
}

#[derive(Component)]
struct ScoreText;

#[cfg(feature = "client")]
fn display_score(
    mut score_text: Query<&mut Text, With<ScoreText>>,
    hits: Query<&Score, With<Replicated>>,
) {
    if let Ok(score) = hits.single() {
        if let Ok(mut text) = score_text.single_mut() {
            text.0 = format!("Score: {}", score.0);
        }
    }
}

#[cfg(feature = "server")]
fn draw_aabb_envelope(query: Query<&ColliderAabb, With<AabbEnvelopeHolder>>, mut gizmos: Gizmos) {
    query.iter().for_each(|collider_aabb| {
        gizmos.rect_2d(
            Isometry2d::new(collider_aabb.center(), Rot2::default()),
            collider_aabb.size(),
            Color::WHITE,
        );
    })
}

/// Add visuals to newly spawned players
fn add_player_visuals(
    trigger: On<Add, PlayerId>,
    query: Query<(Has<Predicted>, &ColorComponent), Without<BulletMarker>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((is_predicted, color)) = query.get(trigger.entity) {
        commands.entity(trigger.entity).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Rectangle::from_length(PLAYER_SIZE)))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
        ));
        if is_predicted {
            commands
                .entity(trigger.entity)
                .insert(FrameInterpolate::<Transform>::default());
        }
    }
}

/// Add visuals to newly spawned bullets
fn add_bullet_visuals(
    trigger: On<Add, BulletMarker>,
    query: Query<
        (&ColorComponent, &Position, &Rotation, Has<Interpolated>),
        (With<BulletMarker>, Without<Mesh2d>),
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((color, position, rotation, interpolated)) = query.get(trigger.entity) {
        commands.entity(trigger.entity).insert((
            Transform::from_translation(position.0.extend(0.0))
                .with_rotation(Quat::from_rotation_z(rotation.as_radians())),
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Circle {
                radius: BULLET_SIZE,
            }))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
        ));
        if !interpolated {
            commands
                .entity(trigger.entity)
                .insert(FrameInterpolate::<Transform>::default());
        }
    }
}

#[cfg(feature = "client")]
fn hide_interpolated_bullets_after_local_hit(
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    fixed_time: Res<Time<Fixed>>,
    server: Query<(), With<Server>>,
    bullets: Query<
        (
            Entity,
            &BulletMarker,
            &Position,
            &LinearVelocity,
            &Visibility,
            Has<Predicted>,
            Has<PreSpawned>,
        ),
        With<BulletMarker>,
    >,
    bots: Query<(Entity, &Position), With<InterpolatedBot>>,
) {
    if !server.is_empty() {
        return;
    }

    let hit_distance_sq = (BOT_RADIUS + BULLET_SIZE).powi(2);
    let fixed_delta = fixed_time.delta_secs();
    let tick = timeline.tick();
    for (
        bullet_entity,
        marker,
        bullet_position,
        bullet_velocity,
        visibility,
        is_predicted,
        is_prespawned,
    ) in &bullets
    {
        if is_predicted || is_prespawned || matches!(visibility, Visibility::Hidden) {
            continue;
        }

        let end = bullet_position.0;
        let start = end - bullet_velocity.0 * fixed_delta;
        for (bot_entity, bot_position) in &bots {
            let distance_sq = point_segment_distance_sq(bot_position.0, start, end);
            if distance_sq <= hit_distance_sq {
                commands.entity(bullet_entity).insert(Visibility::Hidden);
                lightyear_debug_event!(
                    DebugCategory::Prediction,
                    DebugSamplePoint::FixedPostUpdate,
                    "FixedPostUpdate",
                    "bullet_local_hide_interpolated_hit",
                    local_tick = tick.0 as i64,
                    bullet = ?bullet_entity,
                    shooter = ?marker.shooter,
                    shooter_bits = marker.shooter.to_bits(),
                    fire_tick = marker.fire_tick.0 as i64,
                    salt = marker.salt as i64,
                    prespawn_hash = marker.prespawn_hash,
                    bot = ?bot_entity,
                    bullet_position = ?bullet_position,
                    bot_position = ?bot_position,
                    distance = distance_sq.sqrt(),
                    "Hide remote bullet after local interpolated-bot hit"
                );
                break;
            }
        }
    }
}

#[cfg(feature = "client")]
fn point_segment_distance_sq(point: Vec2, start: Vec2, end: Vec2) -> f32 {
    let segment = end - start;
    let len_sq = segment.length_squared();
    if len_sq <= f32::EPSILON {
        return point.distance_squared(end);
    }
    let t = ((point - start).dot(segment) / len_sq).clamp(0.0, 1.0);
    point.distance_squared(start + segment * t)
}

/// Add visuals to newly spawned bots
fn add_interpolated_bot_visuals(
    trigger: On<Add, InterpolatedBot>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let entity = trigger.entity;
    // add visibility
    commands.entity(entity).insert((
        Visibility::default(),
        Mesh2d(meshes.add(Mesh::from(Circle { radius: BOT_RADIUS }))),
        MeshMaterial2d(materials.add(ColorMaterial {
            color: GREEN.into(),
            ..Default::default()
        })),
    ));
}

fn add_predicted_bot_visuals(
    trigger: On<Add, PredictedBot>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let entity = trigger.entity;
    // add visibility
    commands.entity(entity).insert((
        Visibility::default(),
        Mesh2d(meshes.add(Mesh::from(Circle { radius: BOT_RADIUS }))),
        MeshMaterial2d(materials.add(ColorMaterial {
            color: BLUE.into(),
            ..Default::default()
        })),
        // predicted entities are updated in FixedUpdate so they need to be visually interpolated
        FrameInterpolate::<Transform>::default(),
    ));
}
