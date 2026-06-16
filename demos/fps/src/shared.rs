use avian3d::prelude::*;
use bevy::prelude::*;
use bevy_ahoy::{CharacterLook, prelude::*};
use lightyear::{avian3d::plugin::AvianReplicationMode, prelude::*};
use lightyear_ahoy::prelude::{AhoyBeiInputPlugin, LightyearAhoyPlugin};

use crate::protocol::*;

pub const WORLD_COLLISION_LAYER: LayerMask = LayerMask(1 << 0);
pub const PLAYER_COLLISION_LAYER: LayerMask = LayerMask(1 << 1);

pub const PLAYER_RADIUS: f32 = 0.45;
pub const PLAYER_HEIGHT: f32 = 1.45;
pub const PLAYER_EYE_HEIGHT: f32 = 1.35;
pub const SPAWN_POINT: Vec3 = Vec3::new(0.0, 2.2, 8.0);

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.add_plugins((
            LightyearAhoyPlugin::default(),
            AhoyBeiInputPlugin::<PlayerInput>::default(),
        ));
        app.add_plugins(lightyear::avian3d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::PositionButInterpolateTransform,
            rollback_resources: true,
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                .disable::<PhysicsTransformPlugin>()
                .disable::<PhysicsInterpolationPlugin>()
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>(),
        )
        .insert_resource(Gravity(Vec3::ZERO));

        app.add_systems(Startup, setup_world_colliders);
    }
}

#[derive(Clone, Copy)]
pub struct WorldBox {
    pub name: &'static str,
    pub translation: Vec3,
    pub size: Vec3,
    pub rotation: Quat,
    pub color: Color,
}

pub fn world_boxes() -> [WorldBox; 7] {
    [
        WorldBox {
            name: "floor",
            translation: Vec3::new(0.0, -0.2, 0.0),
            size: Vec3::new(34.0, 0.4, 34.0),
            rotation: Quat::IDENTITY,
            color: Color::srgb(0.17, 0.18, 0.19),
        },
        WorldBox {
            name: "center ramp",
            translation: Vec3::new(0.0, 1.0, -2.0),
            size: Vec3::new(5.0, 0.35, 14.0),
            rotation: Quat::from_rotation_x(-0.36),
            color: Color::srgb(0.35, 0.50, 0.58),
        },
        WorldBox {
            name: "left bank",
            translation: Vec3::new(-7.0, 1.8, -6.0),
            size: Vec3::new(10.0, 0.35, 8.0),
            rotation: Quat::from_rotation_z(0.55),
            color: Color::srgb(0.27, 0.40, 0.46),
        },
        WorldBox {
            name: "right bank",
            translation: Vec3::new(7.0, 1.8, -6.0),
            size: Vec3::new(10.0, 0.35, 8.0),
            rotation: Quat::from_rotation_z(-0.55),
            color: Color::srgb(0.27, 0.40, 0.46),
        },
        WorldBox {
            name: "mantle block",
            translation: Vec3::new(-4.0, 0.8, 5.0),
            size: Vec3::new(3.0, 1.6, 2.0),
            rotation: Quat::IDENTITY,
            color: Color::srgb(0.38, 0.43, 0.30),
        },
        WorldBox {
            name: "jump block",
            translation: Vec3::new(5.0, 1.2, 5.5),
            size: Vec3::new(2.2, 2.4, 2.2),
            rotation: Quat::IDENTITY,
            color: Color::srgb(0.45, 0.35, 0.28),
        },
        WorldBox {
            name: "reset platform",
            translation: Vec3::new(0.0, 0.45, 12.0),
            size: Vec3::new(5.0, 0.5, 4.0),
            rotation: Quat::IDENTITY,
            color: Color::srgb(0.42, 0.36, 0.28),
        },
    ]
}

pub fn setup_world_colliders(mut commands: Commands) {
    spawn_world_colliders(&mut commands);
}

pub fn spawn_world_colliders(commands: &mut Commands) {
    for world_box in world_boxes() {
        commands.spawn((
            Name::new(world_box.name),
            RigidBody::Static,
            Collider::cuboid(world_box.size.x, world_box.size.y, world_box.size.z),
            CollisionLayers::new(WORLD_COLLISION_LAYER, LayerMask::ALL),
            Position(world_box.translation),
            Rotation(world_box.rotation),
            Transform {
                translation: world_box.translation,
                rotation: world_box.rotation,
                ..default()
            },
        ));
    }
}

#[cfg(feature = "gui")]
pub fn spawn_world_render(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    for world_box in world_boxes() {
        commands.spawn((
            Name::new(world_box.name),
            Mesh3d(meshes.add(Cuboid::new(
                world_box.size.x,
                world_box.size.y,
                world_box.size.z,
            ))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: world_box.color,
                perceptual_roughness: 0.88,
                ..default()
            })),
            Transform {
                translation: world_box.translation,
                rotation: world_box.rotation,
                ..default()
            },
        ));
    }
}

pub fn player_controller() -> CharacterController {
    CharacterController {
        acceleration_hz: 10.0,
        air_acceleration_hz: 100.0,
        speed: 6.5,
        gravity: 23.0,
        friction_hz: 4.0,
        ..default()
    }
}

pub fn player_collider() -> Collider {
    Collider::capsule(PLAYER_RADIUS, PLAYER_HEIGHT)
}

pub fn player_collision_layers() -> CollisionLayers {
    CollisionLayers::new(PLAYER_COLLISION_LAYER, LayerMask::ALL)
}

pub fn player_spawn_point(slot: u64) -> Vec3 {
    let index = slot.saturating_sub(1) as f32;
    SPAWN_POINT + Vec3::new(index * 1.4, 0.0, 0.0)
}

pub fn color_from_id(client_id: PeerId) -> Color {
    let h = ((client_id.to_bits().wrapping_mul(137) % 360) as f32) / 360.0;
    Color::hsl(h, 0.65, 0.58)
}

pub fn player_bundle(
    client_id: PeerId,
    slot: u64,
) -> (
    Name,
    PlayerMarker,
    PlayerId,
    PlayerSlot,
    ColorComponent,
    Position,
    Rotation,
    Transform,
    LinearVelocity,
    CharacterLook,
) {
    let spawn = player_spawn_point(slot);
    (
        Name::new(format!("Player {slot}")),
        PlayerMarker,
        PlayerId(client_id),
        PlayerSlot(slot),
        ColorComponent(color_from_id(client_id)),
        Position(spawn),
        Rotation::IDENTITY,
        Transform::from_translation(spawn),
        LinearVelocity::ZERO,
        CharacterLook::default(),
    )
}

pub fn ahoy_player_bundle() -> (CharacterController, Collider, CollisionLayers) {
    (
        player_controller(),
        player_collider(),
        player_collision_layers(),
    )
}
