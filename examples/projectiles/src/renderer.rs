use crate::protocol::*;
use crate::shared::direction_only::BulletOf;
use avian2d::prelude::*;
use bevy::color::palettes::basic::{GREEN, RED};
use bevy::color::palettes::css::BLUE;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use bevy_enhanced_input::action::{Action, ActionMock};
use bevy_enhanced_input::prelude::{ActionValue, Actions};
use lightyear::input::bei::prelude::InputMarker;
use lightyear::interpolation::Interpolated;
use lightyear::prelude::{
    Client, Confirmed, Controlled, DeterministicPredicted, PreSpawned, Predicted, Replicate,
    Replicated,
};
use lightyear_avian2d::prelude::AabbEnvelopeHolder;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);

        app.add_observer(add_bullet_visuals);
        app.add_observer(add_bullet_visuals_interpolated);
        app.add_observer(add_player_visuals);
        // app.add_observer(add_hitscan_visual);
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
                    hide_meshes: true,
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
            app.add_systems(Update, (display_score, render_hitscan_lines, display_info));
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
        #[cfg(not(feature = "server"))]
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

        commands.spawn((
            Text::new("Mode information"),
            TextFont::from_font_size(20.0),
            TextColor(Color::WHITE.with_alpha(0.7)),
            Node {
                align_self: AlignSelf::Start,
                position_type: PositionType::Absolute,
                top: Val::Px(30.0),
                left: Val::Px(10.0),
                ..default()
            },
            ModeText,
        ));
    }
}

#[derive(Component)]
struct ScoreText;

#[derive(Component)]
struct ModeText;

#[cfg(feature = "client")]
fn display_score(
    mut score_text: Query<&mut Text, With<ScoreText>>,
    score: Single<&Score, (With<Replicated>, With<Controlled>)>,
) {
    if let Ok(mut text) = score_text.single_mut() {
        text.0 = format!("Score: {}", score.0);
    }
}

#[cfg(feature = "client")]
fn display_info(
    mut mode_text: Single<&mut Text, With<ModeText>>,
    mode_query: Single<
        (
            &ProjectileReplicationMode,
            &GameReplicationMode,
            &WeaponType,
        ),
        With<ClientContext>,
    >,
) {
    let (projectile_mode, replication_mode, weapon_type) = mode_query.into_inner();
    mode_text.0 = format!(
        "Weapon: {}\nProjectile Mode: {}\nReplication Mode: {}\nPress Q to cycle weapons\nPress E to cycle replication\nPress R to cycle rooms\nPress Space to shoot",
        weapon_type.name(),
        projectile_mode.name(),
        replication_mode.name(),
    );
}

#[cfg(feature = "client")]
fn render_hitscan_lines(query: Query<(&HitscanVisual, &ColorComponent)>, mut gizmos: Gizmos) {
    for (visual, color) in query.iter() {
        let progress = visual.lifetime / visual.max_lifetime;
        let alpha = (1.0 - progress).max(0.0);
        let line_color = color.0.with_alpha(alpha);

        gizmos.line_2d(visual.start, visual.end, line_color);
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

// TODO: interpolated players are not visible because components are not inserted at the same time?
/// Add visuals to newly spawned players
fn add_player_visuals(
    trigger: On<Insert, PlayerId>,
    mut query: Query<
        (
            Has<Predicted>,
            Has<DeterministicPredicted>,
            Has<PreSpawned>,
            Has<Interpolated>,
            &mut ColorComponent,
        ),
        With<PlayerMarker>,
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((is_predicted, is_det_predicted, prespawned, interpolated, mut color)) =
        query.get_mut(trigger.entity)
    {
        if interpolated {
            let hsva = Hsva {
                saturation: 0.7,
                ..Hsva::from(color.0)
            };
            color.0 = Color::from(hsva);
        }
        if is_predicted || is_det_predicted || prespawned {
            let hsva = Hsva {
                saturation: 0.4,
                ..Hsva::from(color.0)
            };
            color.0 = Color::from(hsva);
            commands.entity(trigger.entity).insert((
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
            ));
        }
        commands.entity(trigger.entity).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Rectangle::from_length(PLAYER_SIZE)))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
        ));
    }
}

/// Add visuals to newly spawned bullets
fn add_bullet_visuals(
    trigger: On<Add, BulletMarker>,
    // Hitscan are rendered differently
    query: Query<(&ColorComponent, Has<BulletOf>), (Without<HitscanVisual>, With<Position>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((color, has_bullet_of)) = query.get(trigger.entity) {
        // TODO: for interpolation, we want to only start showing the bullet when the Position component gets synced to Interpolated.
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
        // we know that the entity is predicted since
        // - it cannot be interpolated because Position is added later on, not immediately on sync
        // - it cannot be a BulletOf
        if !has_bullet_of {
            commands.entity(trigger.entity).insert((
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
            ));
        }
    }
}

/// Add visuals to newly spawned bullets
///
/// For interpolation, we want to only start showing the bullet when the Position component gets synced to Interpolated.
/// (otherwise it would first appear in the middle of the screen)
fn add_bullet_visuals_interpolated(
    trigger: On<Add, Position>,
    // Hitscan are rendered differently
    query: Query<
        &ColorComponent,
        (
            With<Interpolated>,
            Without<HitscanVisual>,
            With<Position>,
            With<BulletMarker>,
        ),
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok(color) = query.get(trigger.entity) {
        // TODO: for interpolation, we want to only start showing the bullet when the Position component gets synced to Interpolated.
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
    }
}

/// Add visuals to hitscan effects
fn add_hitscan_visual(
    trigger: On<Add, HitscanVisual>,
    query: Query<(&HitscanVisual, &ColorComponent)>,
    mut commands: Commands,
) {
    if let Ok((visual, color)) = query.get(trigger.entity) {
        info!("Add hitscan vis");
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
