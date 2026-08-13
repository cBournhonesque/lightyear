#[cfg(feature = "server")]
use crate::hit_detection::server_rewound::LagCompensatedSilhouette;
use crate::hit_detection::{HitImpact, HitPolicy};
use crate::protocol::*;
use crate::representation::RepresentationKind;
use crate::shared::PLAYER_SIZE;
use crate::timeline::TimelinePolicy;
use crate::trajectory::{TrajectoryKind, hitscan::HitscanVisual};
use avian2d::prelude::*;
use bevy::prelude::*;
use bevy_enhanced_input::action::{Action, mock::ActionMock};
use bevy_enhanced_input::prelude::ActionValue;
use lightyear::interpolation::Interpolated;
use lightyear::prelude::*;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);

        app.add_observer(add_bullet_visuals);
        app.add_systems(Update, add_player_visuals);
        app.add_observer(add_hitscan_visual);
        app.add_systems(PostUpdate, draw_hit_impacts);

        if !app.is_plugin_added::<FrameInterpolationPlugin>() {
            app.add_plugins(FrameInterpolationPlugin);
        }

        #[cfg(feature = "client")]
        {
            app.add_systems(
                PreUpdate,
                // mock the action before BEI evaluates it. BEI evaluated actions mocks in FixedPreUpdate
                update_cursor_state_from_window,
            );
            app.add_systems(Update, (display_score, render_hitscan_lines, display_info));
        }

        #[cfg(feature = "server")]
        {
            app.add_systems(PostUpdate, draw_lag_compensated_silhouettes);
        }
    }
}

/// Draw a bright cross at the exact collision point.
///
/// `HitImpact` exists only in the app that performed the query, so this also
/// makes hit authority immediately visible without adding network messages.
fn draw_hit_impacts(impacts: Query<&HitImpact>, mut gizmos: Gizmos) {
    const CROSS_HALF_SIZE: f32 = 10.0;

    for impact in &impacts {
        let normal = impact.normal.try_normalize().unwrap_or(Vec2::Y);
        let tangent = Vec2::new(-normal.y, normal.x);
        let cross_color = Color::srgb(1.0, 0.15, 0.1);

        gizmos.line_2d(
            impact.position - tangent * CROSS_HALF_SIZE,
            impact.position + tangent * CROSS_HALF_SIZE,
            cross_color,
        );
        gizmos.line_2d(
            impact.position - normal * CROSS_HALF_SIZE,
            impact.position + normal * CROSS_HALF_SIZE,
            cross_color,
        );
    }
}

/// Compute the world-position of the cursor and set it in the DualAxis input
fn update_cursor_state_from_window(
    window: Single<&Window>,
    q_camera: Query<(&Camera, &GlobalTransform)>,
    mut action_query: Query<&mut ActionMock, With<Action<MoveCursor>>>,
) {
    let Ok((camera, camera_transform)) = q_camera.single() else {
        error!("Expected to find only one camera");
        return;
    };
    if let Some(world_position) = window
        .cursor_position()
        .and_then(|cursor| Some(camera.viewport_to_world(camera_transform, cursor).unwrap()))
        .map(|ray| ray.origin.truncate())
    {
        for mut action_mock in action_query.iter_mut() {
            action_mock.value = ActionValue::Axis2D(world_position);
        }
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
    #[cfg(feature = "client")]
    {
        commands
            .spawn((
                Node {
                    width: Val::Px(460.0),
                    position_type: PositionType::Absolute,
                    top: Val::Px(10.0),
                    right: Val::Px(10.0),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::FlexEnd,
                    row_gap: Val::Px(6.0),
                    padding: UiRect::all(Val::Px(10.0)),
                    ..default()
                },
                BackgroundColor(Color::BLACK.with_alpha(0.45)),
            ))
            .with_children(|parent| {
                parent.spawn((
                    Text::new("Score: 0"),
                    TextFont::from_font_size(24.0),
                    TextColor(Color::WHITE.with_alpha(0.75)),
                    ScoreText,
                ));

                parent.spawn((
                    Text::new("Waiting for mode information"),
                    TextFont::from_font_size(16.0),
                    TextColor(Color::WHITE.with_alpha(0.85)),
                    Node {
                        width: Val::Px(440.0),
                        ..default()
                    },
                    ModeText,
                ));
            });
    }
}

#[derive(Component)]
struct ScoreText;

#[derive(Component)]
struct ModeText;

#[cfg(feature = "client")]
fn display_score(
    mut score_text: Query<&mut Text, With<ScoreText>>,
    clients: Query<&LocalId, With<Client>>,
    scores: Query<
        (
            &PlayerId,
            &Score,
            Has<ControlledBy>,
            Has<Predicted>,
            Has<Interpolated>,
        ),
        With<PlayerMarker>,
    >,
) {
    let Ok(mut text) = score_text.single_mut() else {
        return;
    };
    let Ok(local_id) = clients.single() else {
        return;
    };

    // In a host-client world the authoritative and presentation entities can
    // share a PlayerId. Prefer the authoritative copy there; otherwise use the
    // local predicted/interpolated presentation copy. This remains correct now
    // that the score can both increase and decrease.
    let mut presentation_score = None;
    let mut fallback_score = None;
    let mut authoritative_score = None;
    for (player_id, score, authoritative, predicted, interpolated) in &scores {
        if player_id.0 != local_id.0 {
            continue;
        }
        fallback_score = Some(score.0);
        if predicted || interpolated {
            presentation_score = Some(score.0);
        }
        if authoritative {
            authoritative_score = Some(score.0);
        }
    }
    let score = authoritative_score
        .or(presentation_score)
        .or(fallback_score)
        .unwrap_or(0);
    text.0 = format!("Score: {score}");
}

#[cfg(feature = "client")]
fn display_info(
    mut mode_text: Query<&mut Text, With<ModeText>>,
    mode_query: Query<
        (
            &TrajectoryKind,
            &RepresentationKind,
            &HitPolicy,
            &TimelinePolicy,
        ),
        With<ClientContext>,
    >,
) {
    let Ok(mut mode_text) = mode_text.single_mut() else {
        return;
    };
    let Ok((trajectory, representation, hit_policy, timeline)) = mode_query.single() else {
        mode_text.0 = "Waiting for mode information".to_string();
        return;
    };
    mode_text.0 = format!(
        "Trajectory: {}\nRepresentation: {}\nHit policy: {}\nTimeline: {}\nQ: trajectory  E: representation\nR: hit policy  T: timeline\nSpace: shoot",
        trajectory.name(),
        representation.name(),
        hit_policy.name(),
        timeline.name(),
    );
}

#[cfg(feature = "client")]
fn render_hitscan_lines(query: Query<(&HitscanVisual, &ColorComponent)>, mut gizmos: Gizmos) {
    for (visual, color) in query.iter() {
        // A state-entity trace stays alive long enough for owner prespawn
        // matching, but it should not remain visually bright for that entire
        // network lifetime.
        let fade_lifetime = visual
            .max_lifetime
            .min(crate::trajectory::hitscan::LOCAL_VISUAL_LIFETIME);
        let progress = visual.lifetime / fade_lifetime;
        let alpha = (1.0 - progress).max(0.0);
        let line_color = color.0.with_alpha(alpha);
        gizmos.line_2d(visual.start, visual.end, line_color);
    }
}

/// Draw only the exact historical collider pose tested by the rewound query.
///
/// The lag-compensation broad-phase AABB is an implementation detail and is
/// intentionally hidden. This yellow outline is the target silhouette that
/// matters when explaining either a hit or a miss.
#[cfg(feature = "server")]
fn draw_lag_compensated_silhouettes(
    silhouettes: Query<&LagCompensatedSilhouette>,
    mut gizmos: Gizmos,
) {
    for silhouette in &silhouettes {
        let color = Color::srgb(1.0, 0.82, 0.1);
        gizmos.rect_2d(
            Isometry2d::new(silhouette.position, Rot2::radians(silhouette.rotation)),
            Vec2::splat(PLAYER_SIZE),
            color,
        );
    }
}

/// Add visuals to newly spawned players
fn add_player_visuals(
    mut query: Query<
        (
            Entity,
            Has<Predicted>,
            Has<PreSpawned>,
            Has<Interpolated>,
            Has<Bot>,
            &mut ColorComponent,
        ),
        // Same thing, for interpolation, make sure that both Position and Rotation
        // are present! Otherwise the Mesh will insert Transform::default()
        (
            With<PlayerMarker>,
            With<PlayerId>,
            With<Position>,
            With<Rotation>,
            Without<Mesh2d>,
        ),
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    for (entity, is_predicted, prespawned, interpolated, is_bot, mut color) in &mut query {
        let mut visual_color = color.0;
        if interpolated {
            let hsva = Hsva {
                saturation: 0.7,
                ..Hsva::from(color.0)
            };
            color.0 = Color::from(hsva);
            visual_color = color.0;
        }
        if is_predicted || prespawned {
            let hsva = Hsva {
                saturation: 0.4,
                ..Hsva::from(color.0)
            };
            color.0 = Color::from(hsva);
            visual_color = color.0;
            commands.entity(entity).insert(FrameInterpolate);
        }
        if is_bot {
            visual_color = Color::srgb(1.0, 0.85, 0.1);
        }
        // Keep the mesh the same size as the collision rectangle. The old bot
        // mesh was 20% larger, which made exact edge impacts look inset.
        let size = PLAYER_SIZE;
        commands.entity(entity).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Rectangle::from_length(size)))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: visual_color,
                ..Default::default()
            })),
        ));
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::Update,
            "Update",
            "projectiles_player_visual_added",
            entity = ?entity,
            is_predicted = is_predicted,
            is_prespawned = prespawned,
            is_interpolated = interpolated,
            is_bot = is_bot,
            color = ?visual_color,
            "Projectiles player visual added"
        );
    }
}

/// Add visuals to newly spawned bullets
fn add_bullet_visuals(
    trigger: On<Add, (Position, Rotation)>,
    // Hitscan are rendered differently
    query: Query<
        (&ColorComponent, &Position, &Rotation, Has<Interpolated>),
        (Without<HitscanVisual>, With<BulletMarker>, Without<Mesh2d>),
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((color, position, rotation, interpolated)) = query.get(trigger.entity) {
        commands.entity(trigger.entity).insert((
            // State-Entity replication can insert Position/Rotation before Interpolated.
            // Interpolation then moves that pose into history and removes the live
            // Position/Rotation, but it does not remove Transform. Seed Transform from
            // the transient pose so the bullet stays at its correct spawn position until
            // the presentation tick restores Position/Rotation and normal sync resumes.
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
        // if not interpolated, then the entity gets updated in FixedUpdate and needs
        // FrameInterpolation to be smooth
        if !interpolated {
            commands.entity(trigger.entity).insert(FrameInterpolate);
        }
    }
}

/// Add visuals to hitscan effects
fn add_hitscan_visual(
    trigger: On<Add, HitscanVisual>,
    query: Query<(&HitscanVisual, &ColorComponent)>,
    mut commands: Commands,
) {
    if let Ok((visual, color)) = query.get(trigger.entity) {
        // For now, we'll use gizmos to draw the line in a separate system
        // This is a simple implementation; in a real game you might want
        // more sophisticated line rendering
        commands
            .entity(trigger.entity)
            .insert((Visibility::default(), Name::new("HitscanLine")));
    }
}
