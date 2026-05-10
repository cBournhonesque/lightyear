use crate::protocol::*;
use crate::shared::BOT_RADIUS;
use avian2d::prelude::*;
use bevy::color::palettes::basic::GREEN;
use bevy::color::palettes::css::BLUE;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use lightyear::connection::host::HostServer;
use lightyear::interpolation::Interpolated;
use lightyear::prelude::{
    lightyear_debug_event, Client, DebugCategory, DebugSamplePoint, InputTimeline,
    InterpolationTimeline, IsSynced, LocalTimeline, PreSpawned, Predicted, Replicate, Replicated,
    Server,
};
use lightyear_avian2d::prelude::AabbEnvelopeHolder;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);

        app.add_systems(
            PostUpdate,
            (
                add_player_visuals,
                add_interpolated_bot_visuals,
                add_predicted_bot_visuals,
                add_bullet_visuals,
                emit_bullet_visual_state,
            )
                .after(TransformSystems::Propagate),
        );
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

fn client_visuals_ready(
    client: &Query<(), With<Client>>,
    host_server: &Query<(), With<HostServer>>,
    input_synced: &Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    interpolation_synced: &Query<(), (With<Client>, With<IsSynced<InterpolationTimeline>>)>,
) -> bool {
    client.is_empty()
        || !host_server.is_empty()
        || (!input_synced.is_empty() && !interpolation_synced.is_empty())
}

/// Add visuals to newly spawned players after their replicated transform is ready.
fn add_player_visuals(
    query: Query<
        (Entity, Has<Predicted>, &ColorComponent, &Transform),
        (
            With<PlayerId>,
            With<PlayerMarker>,
            With<GlobalTransform>,
            Without<BulletMarker>,
            Without<Mesh2d>,
        ),
    >,
    client: Query<(), With<Client>>,
    host_server: Query<(), With<HostServer>>,
    input_synced: Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    interpolation_synced: Query<(), (With<Client>, With<IsSynced<InterpolationTimeline>>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if !client_visuals_ready(&client, &host_server, &input_synced, &interpolation_synced) {
        return;
    }
    for (entity, is_predicted, color, transform) in &query {
        commands.entity(entity).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Rectangle::from_length(PLAYER_SIZE)))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
        ));
        if is_predicted {
            commands
                .entity(entity)
                .insert(FrameInterpolate::<Transform>::default());
        }
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "player_visual_added",
            entity = ?entity,
            transform = ?transform.translation.truncate(),
            is_predicted = is_predicted,
            "Player visual added after transform propagation"
        );
    }
}

fn add_bullet_visuals(
    query: Query<
        (
            Entity,
            &BulletMarker,
            &ColorComponent,
            &Position,
            &Rotation,
            &Transform,
            Has<Interpolated>,
            Has<Predicted>,
            Has<PreSpawned>,
            Has<Replicate>,
        ),
        (With<BulletMarker>, With<GlobalTransform>, Without<Mesh2d>),
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    for (
        entity,
        marker,
        color,
        position,
        rotation,
        transform,
        is_interpolated,
        is_predicted,
        is_prespawned,
        is_replicate,
    ) in &query
    {
        if !is_predicted && !is_prespawned && !is_interpolated && !is_replicate {
            continue;
        }

        let mut entity_commands = commands.entity(entity);
        entity_commands.insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Circle {
                radius: BULLET_SIZE,
            }))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
        ));
        if is_predicted || is_prespawned {
            entity_commands.insert(FrameInterpolate::<Transform>::default());
        }

        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "bullet_visual_added",
            entity = ?entity,
            shooter = ?marker.shooter,
            shooter_bits = marker.shooter.to_bits(),
            fire_tick = marker.fire_tick.0 as i64,
            salt = marker.salt as i64,
            prespawn_hash = marker.prespawn_hash,
            position = ?position,
            rotation = ?rotation,
            transform = ?transform.translation.truncate(),
            is_interpolated = is_interpolated,
            is_predicted = is_predicted,
            is_prespawned = is_prespawned,
            "Bullet visual added after transform propagation"
        );
    }
}

fn emit_bullet_visual_state(
    timeline: Res<LocalTimeline>,
    query: Query<
        (
            Entity,
            &BulletMarker,
            &Position,
            &Transform,
            &GlobalTransform,
            Option<&Visibility>,
            Option<&FrameInterpolate<Transform>>,
            Has<Interpolated>,
            Has<Predicted>,
            Has<PreSpawned>,
        ),
        With<BulletMarker>,
    >,
) {
    let tick = timeline.tick();
    for (
        entity,
        marker,
        position,
        transform,
        global_transform,
        visibility,
        frame_interpolate,
        is_interpolated,
        is_predicted,
        is_prespawned,
    ) in &query
    {
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "fps_bullet_visual_state",
            local_tick = tick.0 as i64,
            entity = ?entity,
            shooter = ?marker.shooter,
            shooter_bits = marker.shooter.to_bits(),
            fire_tick = marker.fire_tick.0 as i64,
            salt = marker.salt as i64,
            prespawn_hash = marker.prespawn_hash,
            position = ?position,
            transform = ?transform.translation.truncate(),
            global_transform = ?global_transform.translation().truncate(),
            visibility = ?visibility,
            has_frame_interpolate = frame_interpolate.is_some(),
            frame_interpolate = ?frame_interpolate,
            is_interpolated = is_interpolated,
            is_predicted = is_predicted,
            is_prespawned = is_prespawned,
            "FPS bullet visual state after transform propagation"
        );
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
    query: Query<
        (Entity, &Transform),
        (
            With<InterpolatedBot>,
            With<GlobalTransform>,
            Without<Mesh2d>,
        ),
    >,
    client: Query<(), With<Client>>,
    host_server: Query<(), With<HostServer>>,
    input_synced: Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    interpolation_synced: Query<(), (With<Client>, With<IsSynced<InterpolationTimeline>>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if !client_visuals_ready(&client, &host_server, &input_synced, &interpolation_synced) {
        return;
    }
    for (entity, transform) in &query {
        commands.entity(entity).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Circle { radius: BOT_RADIUS }))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: GREEN.into(),
                ..Default::default()
            })),
        ));
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "interpolated_bot_visual_added",
            entity = ?entity,
            transform = ?transform.translation.truncate(),
            "Interpolated bot visual added after transform propagation"
        );
    }
}

fn add_predicted_bot_visuals(
    query: Query<
        (Entity, &Transform),
        (With<PredictedBot>, With<GlobalTransform>, Without<Mesh2d>),
    >,
    client: Query<(), With<Client>>,
    host_server: Query<(), With<HostServer>>,
    input_synced: Query<(), (With<Client>, With<IsSynced<InputTimeline>>)>,
    interpolation_synced: Query<(), (With<Client>, With<IsSynced<InterpolationTimeline>>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if !client_visuals_ready(&client, &host_server, &input_synced, &interpolation_synced) {
        return;
    }
    for (entity, transform) in &query {
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
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "predicted_bot_visual_added",
            entity = ?entity,
            transform = ?transform.translation.truncate(),
            "Predicted bot visual added after transform propagation"
        );
    }
}
