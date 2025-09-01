use crate::protocol::*;
use crate::shared::BOT_RADIUS;
use avian2d::prelude::*;
use bevy::color::palettes::basic::GREEN;
use bevy::color::palettes::css::BLUE;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use bevy_enhanced_input::action::{Action, ActionMock};
use bevy_enhanced_input::prelude::{ActionValue, Actions};
use lightyear::input::bei::prelude::InputMarker;
use lightyear::interpolation::Interpolated;
use lightyear::prediction::prespawn::PreSpawned;
use lightyear::prelude::{
    Client, Confirmed, Controlled, DeterministicPredicted, Predicted, Replicate, Replicated,
};
use lightyear_avian2d::prelude::AabbEnvelopeHolder;
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);

        app.add_observer(add_bullet_visuals);
        app.add_observer(add_player_visuals);
        app.add_observer(add_hitscan_visual);
        app.add_observer(add_physics_projectile_visuals);
        app.add_observer(add_homing_missile_visuals);

        app.add_plugins(FrameInterpolationPlugin::<Position>::default());
        // app.add_plugins(FrameInterpolationPlugin::<Rotation>::default());

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
            Text::new(""),
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
    weapon_type: Single<
        &WeaponType,
        (
            With<Actions<PlayerContext>>,
            With<PlayerMarker>,
            With<Controlled>,
        ),
    >,
    mode_query: Single<(&ProjectileReplicationMode, &GameReplicationMode), With<ClientContext>>,
) {
    let (projectile_mode, replication_mode) = mode_query.into_inner();
    mode_text.0 = format!(
        "Weapon: {}\nProjectile Mode: {}\nReplication Mode: {}\nPress Q to cycle weapons\nPress E to cycle replication\nPress R to cycle rooms\nPress Space to shoot",
        weapon_type.name(),
        projectile_mode.name(),
        replication_mode.name(),
    );
}

#[cfg(feature = "client")]
fn render_hitscan_lines(
    query: Query<(&HitscanVisual, &ColorComponent), Without<Confirmed>>,
    mut gizmos: Gizmos,
) {
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

/// Convenient for filter for entities that should be visible
/// Works either on the client or the server
#[derive(QueryFilter)]
pub struct VisibleFilter {
    a: Or<(
        With<Predicted>,
        With<DeterministicPredicted>,
        // to show prespawned entities
        With<PreSpawned>,
        With<Interpolated>,
        // to show entities on the server
        With<Replicate>,
    )>,
    // we don't show any replicated (confirmed) entities unless it's the DeterministicPredicted case
    b: Or<(Without<Replicated>, With<DeterministicPredicted>)>,
}

// TODO: interpolated players are not visible because components are not inserted at the same time?
/// Add visuals to newly spawned players
fn add_player_visuals(
    trigger: Trigger<OnAdd, PlayerId>,
    query: Query<
        (Has<Predicted>, Has<DeterministicPredicted>, &ColorComponent),
        (VisibleFilter, With<PlayerMarker>),
    >,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((is_predicted, is_det_predicted, color)) = query.get(trigger.target()) {
        commands.entity(trigger.target()).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Rectangle::from_length(PLAYER_SIZE)))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
        ));
        if is_predicted || is_det_predicted {
            commands.entity(trigger.target()).insert((
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
            ));
        }
    }
}

/// Add visuals to newly spawned bullets
fn add_bullet_visuals(
    trigger: Trigger<OnAdd, BulletMarker>,
    query: Query<(&ColorComponent, &Position, Has<Interpolated>), VisibleFilter>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((color, position, interpolated)) = query.get(trigger.target()) {
        commands.entity(trigger.target()).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(Circle {
                radius: BULLET_SIZE,
            }))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: color.0,
                ..Default::default()
            })),
        ));
        if interpolated {
            commands.entity(trigger.target()).insert((
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
            ));
        }
    }
}

/// Add visuals to hitscan effects
fn add_hitscan_visual(
    trigger: Trigger<OnAdd, HitscanVisual>,
    query: Query<(&HitscanVisual, &ColorComponent), VisibleFilter>,
    mut commands: Commands,
) {
    if let Ok((visual, color)) = query.get(trigger.target()) {
        // For now, we'll use gizmos to draw the line in a separate system
        // This is a simple implementation; in a real game you might want
        // more sophisticated line rendering
        commands
            .entity(trigger.target())
            .insert((Visibility::default(), Name::new("HitscanLine")));
    }
}

/// Add visuals to physics projectiles (same as bullets but with different color)
fn add_physics_projectile_visuals(
    trigger: Trigger<OnAdd, PhysicsProjectile>,
    query: Query<(&ColorComponent, Has<Interpolated>), (VisibleFilter, With<BulletMarker>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((color, interpolated)) = query.get(trigger.target()) {
        // Make physics projectiles slightly larger and more orange
        let physics_color = Color::srgb(1.0, 0.5, 0.0); // Orange color for physics projectiles

        commands.entity(trigger.target()).insert((
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
            commands.entity(trigger.target()).insert((
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
            ));
        }
    }
}

/// Add visuals to homing missiles (triangle shape)
fn add_homing_missile_visuals(
    trigger: Trigger<OnAdd, HomingMissile>,
    query: Query<(&ColorComponent, Has<Interpolated>), (VisibleFilter, With<BulletMarker>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if let Ok((color, interpolated)) = query.get(trigger.target()) {
        // Make homing missiles red and triangle-shaped
        let missile_color = Color::srgb(1.0, 0.0, 0.0); // Red color for missiles

        // Create a triangle mesh for the missile
        let triangle = Triangle2d::new(
            Vec2::new(0.0, BULLET_SIZE * 2.0),     // Top point
            Vec2::new(-BULLET_SIZE, -BULLET_SIZE), // Bottom left
            Vec2::new(BULLET_SIZE, -BULLET_SIZE),  // Bottom right
        );

        commands.entity(trigger.target()).insert((
            Visibility::default(),
            Mesh2d(meshes.add(Mesh::from(triangle))),
            MeshMaterial2d(materials.add(ColorMaterial {
                color: missile_color,
                ..Default::default()
            })),
        ));
        if !interpolated {
            commands.entity(trigger.target()).insert((
                FrameInterpolate::<Position>::default(),
                FrameInterpolate::<Rotation>::default(),
            ));
        }
    }
}
