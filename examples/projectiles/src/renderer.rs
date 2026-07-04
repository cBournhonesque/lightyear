use crate::protocol::*;
use crate::shared::direction_only::BulletOf;
use avian2d::prelude::*;
use bevy::color::palettes::basic::{GREEN, RED};
use bevy::color::palettes::css::BLUE;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use bevy_enhanced_input::action::{Action, mock::ActionMock};
use bevy_enhanced_input::prelude::{ActionValue, Actions};
use lightyear::input::bei::prelude::InputMarker;
use lightyear::interpolation::Interpolated;
use lightyear::prelude::*;
use lightyear_avian2d::prelude::AabbEnvelopeHolder;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);

        app.add_observer(add_bullet_visuals);
        app.add_systems(Update, add_player_visuals);
        app.add_observer(add_hitscan_visual);
        app.add_observer(add_physics_projectile_visuals);
        app.add_observer(add_homing_missile_visuals);

        app.add_plugins(FrameInterpolationPlugin::<Position>::default());
        app.add_plugins(FrameInterpolationPlugin::<Rotation>::default());

        app.add_plugins(PhysicsDebugPlugin::default())
            .insert_gizmo_config(
                PhysicsGizmos {
                    // aabb_color: Some(Color::WHITE),
                    collider_color: Some(BLUE.into()),
                    raycast_color: Some(GREEN.into()),
                    raycast_point_color: Some(RED.into()),
                    raycast_normal_color: Some(RED.into()),
                    hide_meshes: false,
                    ..default()
                },
                GizmoConfig::default(),
            );

        #[cfg(feature = "client")]
        {
            app.add_systems(
                PreUpdate,
                // mock the action before BEI evaluates it. BEI evaluated actions mocks in FixedPreUpdate
                update_cursor_state_from_window,
            );
            app.add_systems(
                Update,
                (
                    display_score,
                    render_hitscan_lines,
                    display_info,
                    sync_active_mode_visibility.after(add_player_visuals),
                ),
            );
        }

        #[cfg(feature = "server")]
        {
            app.add_systems(PostUpdate, draw_aabb_envelope);
        }
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
                    TextFont::from_font_size(30.0),
                    TextColor(Color::WHITE.with_alpha(0.75)),
                    ScoreText,
                ));

                parent.spawn((
                    Text::new("Waiting for mode information"),
                    TextFont::from_font_size(20.0),
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
    active_mode: Query<&GameReplicationMode, With<ClientContext>>,
    scores: Query<(&Score, &GameReplicationMode), (With<PlayerMarker>, With<Controlled>)>,
) {
    let Ok(mut text) = score_text.single_mut() else {
        return;
    };
    let Ok(active_mode) = active_mode.single() else {
        text.0 = "Score: 0".to_string();
        return;
    };
    if let Some((score, _)) = scores.iter().find(|(_, mode)| *mode == active_mode) {
        text.0 = format!("Score: {}", score.0);
    }
}

#[cfg(feature = "client")]
fn display_info(
    mut mode_text: Query<&mut Text, With<ModeText>>,
    mode_query: Query<
        (
            &ProjectileReplicationMode,
            &GameReplicationMode,
            &WeaponType,
        ),
        With<ClientContext>,
    >,
) {
    let Ok(mut mode_text) = mode_text.single_mut() else {
        return;
    };
    let Ok((projectile_mode, replication_mode, weapon_type)) = mode_query.single() else {
        mode_text.0 = "Waiting for mode information".to_string();
        return;
    };
    mode_text.0 = format!(
        "Weapon: {}\nProjectile Mode: {}\nReplication Mode: {}\nPress Q to cycle weapons\nPress E to cycle projectiles\nPress R to cycle replication\nPress Space to shoot",
        weapon_type.name(),
        projectile_mode.name(),
        replication_mode.name(),
    );
}

fn is_active_mode(mode: Option<&GameReplicationMode>, active_mode: &GameReplicationMode) -> bool {
    mode.is_some_and(|mode| mode == active_mode)
}

#[cfg(feature = "client")]
fn render_hitscan_lines(
    active_mode: Query<&GameReplicationMode, With<ClientContext>>,
    shooters: Query<&GameReplicationMode, With<PlayerMarker>>,
    query: Query<(&HitscanVisual, &ColorComponent, &BulletMarker)>,
    mut gizmos: Gizmos,
) {
    let Ok(active_mode) = active_mode.single() else {
        return;
    };
    for (visual, color, marker) in query.iter() {
        if !is_active_mode(shooters.get(marker.shooter).ok(), active_mode) {
            continue;
        }
        let progress = visual.lifetime / visual.max_lifetime;
        let alpha = (1.0 - progress).max(0.0);
        let line_color = color.0.with_alpha(alpha);
        gizmos.line_2d(visual.start, visual.end, line_color);
    }
}

#[cfg(feature = "client")]
fn sync_active_mode_visibility(
    active_mode: Query<&GameReplicationMode, With<ClientContext>>,
    shooters: Query<&GameReplicationMode, With<PlayerMarker>>,
    mut players: Query<
        (&GameReplicationMode, &mut Visibility),
        (With<PlayerMarker>, With<PlayerId>, Without<BulletMarker>),
    >,
    mut projectiles: Query<(&BulletMarker, &mut Visibility), Without<PlayerMarker>>,
) {
    let Ok(active_mode) = active_mode.single() else {
        return;
    };

    for (mode, mut visibility) in &mut players {
        *visibility = if mode == active_mode {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }

    for (marker, mut visibility) in &mut projectiles {
        *visibility = if is_active_mode(shooters.get(marker.shooter).ok(), active_mode) {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
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
    mut query: Query<
        (
            Entity,
            Has<Predicted>,
            Has<DeterministicPredicted>,
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
    for (entity, is_predicted, is_det_predicted, prespawned, interpolated, is_bot, mut color) in
        &mut query
    {
        let mut visual_color = color.0;
        if interpolated {
            let hsva = Hsva {
                saturation: 0.7,
                ..Hsva::from(color.0)
            };
            color.0 = Color::from(hsva);
            visual_color = color.0;
        }
        if is_predicted || is_det_predicted || prespawned {
            let hsva = Hsva {
                saturation: 0.4,
                ..Hsva::from(color.0)
            };
            color.0 = Color::from(hsva);
            visual_color = color.0;
            commands.entity(entity).insert((
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
            ));
        }
        let size = if is_bot {
            visual_color = Color::srgb(1.0, 0.85, 0.1);
            PLAYER_SIZE * 1.2
        } else {
            PLAYER_SIZE
        };
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
            is_deterministic_predicted = is_det_predicted,
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
        (&ColorComponent, Has<Interpolated>),
        (
            Without<HitscanVisual>,
            With<Position>,
            // only add Transform when BOTH Position/Rotation are present
            // otherwise the Transform will not get synced and the entity will
            // appear in the middle of the screen
            // This can happen because Rotation is added later than Position for
            // interpolated bullets.
            With<Rotation>,
            With<BulletMarker>,
            Without<Mesh2d>,
        ),
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((color, interpolated)) = query.get(trigger.entity) {
        commands.entity(trigger.entity).insert((
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
            commands.entity(trigger.entity).insert((
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
            ));
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

/// Add visuals to physics projectiles (same as bullets but with different color)
fn add_physics_projectile_visuals(
    trigger: On<Add, PhysicsProjectile>,
    query: Query<(&ColorComponent, Has<Interpolated>), With<BulletMarker>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((color, interpolated)) = query.get(trigger.entity) {
        // Make physics projectiles slightly larger and more orange
        let physics_color = Color::srgb(1.0, 0.5, 0.0); // Orange color for physics projectiles

        commands.entity(trigger.entity).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Circle {
                radius: BULLET_SIZE * 1.2, // Slightly larger
            }))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: physics_color,
                ..Default::default()
            })),
        ));
        if !interpolated {
            commands.entity(trigger.entity).insert((
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
            ));
        }
    }
}

/// Add visuals to homing missiles (triangle shape)
fn add_homing_missile_visuals(
    trigger: On<Add, HomingMissile>,
    query: Query<(&ColorComponent, Has<Interpolated>), With<BulletMarker>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((color, interpolated)) = query.get(trigger.entity) {
        // Make homing missiles red and triangle-shaped
        let missile_color = Color::srgb(1.0, 0.0, 0.0); // Red color for missiles

        // Create a triangle mesh for the missile
        let triangle = Triangle2d::new(
            Vec2::new(0.0, BULLET_SIZE * 2.0),     // Top point
            Vec2::new(-BULLET_SIZE, -BULLET_SIZE), // Bottom left
            Vec2::new(BULLET_SIZE, -BULLET_SIZE),  // Bottom right
        );

        commands.entity(trigger.entity).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(triangle))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: missile_color,
                ..Default::default()
            })),
        ));
        if !interpolated {
            commands.entity(trigger.entity).insert((
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
            ));
        }
    }
}
