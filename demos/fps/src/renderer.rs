use avian3d::prelude::*;
use bevy::{
    input::common_conditions::input_just_pressed,
    prelude::*,
    window::{CursorGrabMode, CursorOptions},
};
use bevy_ahoy::CharacterLook;
use lightyear::prelude::*;
use lightyear_frame_interpolation::FrameInterpolate;

use crate::{
    protocol::*,
    shared::{PLAYER_EYE_HEIGHT, PLAYER_HEIGHT, PLAYER_RADIUS, SPAWN_POINT, spawn_world_render},
};

#[derive(Component)]
struct PlayerVisual;

pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_scene);
        app.add_systems(
            Update,
            (
                capture_cursor.run_if(input_just_pressed(MouseButton::Left)),
                release_cursor.run_if(input_just_pressed(KeyCode::Escape)),
                add_player_cosmetics,
                update_local_visibility,
                update_camera,
            )
                .chain(),
        );
        app.add_observer(add_frame_interpolation);
    }
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 400.0,
        ..default()
    });

    commands.spawn((
        DirectionalLight {
            illuminance: 18_000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(-6.0, 12.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    spawn_world_render(&mut commands, &mut meshes, &mut materials);

    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 80.0_f32.to_radians(),
            ..default()
        }),
        Transform::from_translation(SPAWN_POINT + Vec3::new(0.0, 2.5, 5.0))
            .looking_at(SPAWN_POINT, Vec3::Y),
    ));
}

fn add_frame_interpolation(trigger: On<Add, Interpolated>, mut commands: Commands) {
    commands.entity(trigger.entity).insert((
        FrameInterpolate::<Position> {
            trigger_change_detection: true,
            ..default()
        },
        FrameInterpolate::<Rotation> {
            trigger_change_detection: true,
            ..default()
        },
    ));
}

fn add_player_cosmetics(
    mut commands: Commands,
    players: Query<
        (Entity, &ColorComponent),
        (
            Or<(Added<Predicted>, Added<Interpolated>, Added<Replicate>)>,
            With<PlayerMarker>,
            Without<PlayerVisual>,
        ),
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, color) in &players {
        commands.entity(entity).insert((
            Mesh3d(meshes.add(Capsule3d::new(PLAYER_RADIUS, PLAYER_HEIGHT))),
            MeshMaterial3d(materials.add(color.0)),
            PlayerVisual,
        ));
    }
}

fn update_local_visibility(
    mut players: Query<&mut Visibility, (With<PlayerVisual>, With<Controlled>)>,
) {
    for mut visibility in &mut players {
        *visibility = Visibility::Hidden;
    }
}

fn capture_cursor(mut cursor: Single<&mut CursorOptions>) {
    cursor.grab_mode = CursorGrabMode::Locked;
    cursor.visible = false;
}

fn release_cursor(mut cursor: Single<&mut CursorOptions>) {
    cursor.grab_mode = CursorGrabMode::None;
    cursor.visible = true;
}

fn update_camera(
    mut camera: Single<&mut Transform, (With<Camera3d>, Without<PlayerMarker>)>,
    players: Query<
        (&Transform, &CharacterLook),
        (With<PlayerMarker>, With<Predicted>, With<Controlled>),
    >,
) {
    let Ok((player_transform, look)) = players.single() else {
        return;
    };

    camera.translation = player_transform.translation + Vec3::Y * PLAYER_EYE_HEIGHT;
    camera.rotation = Quat::from_euler(EulerRot::YXZ, look.yaw, look.pitch, 0.0);
}
