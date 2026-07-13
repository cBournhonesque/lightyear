use crate::protocol::*;
use avian2d::prelude::*;
use bevy::prelude::*;
use core::hash::{Hash, Hasher};
use leafwing_input_manager::prelude::ActionState;
use lightyear::avian2d::plugin::AvianReplicationMode;
use lightyear::prelude::*;

pub(crate) const MAX_VELOCITY: f32 = 200.0;
const WALL_SIZE: f32 = 350.0;

#[derive(Bundle)]
pub(crate) struct PlayerBodyBundle {
    rigid_body: RigidBody,
}

impl PlayerBodyBundle {
    pub(crate) fn dynamic() -> Self {
        Self {
            rigid_body: RigidBody::Dynamic,
        }
    }
}

impl PlayerPartKind {
    pub(crate) fn local_transform(self) -> Transform {
        match self {
            Self::Hull => {
                Transform::from_xyz(-5.0, -3.0, 0.0).with_rotation(Quat::from_rotation_z(-0.12))
            }
            Self::Pivot => {
                Transform::from_xyz(10.0, 4.0, 0.0).with_rotation(Quat::from_rotation_z(0.35))
            }
            Self::Nose => {
                Transform::from_xyz(6.0, 1.0, 0.0).with_rotation(Quat::from_rotation_z(0.2))
            }
            Self::Sensor => Transform::from_xyz(-7.0, 8.0, 0.0),
        }
    }

    pub(crate) fn collider(self) -> Option<Collider> {
        match self {
            Self::Hull => Some(Collider::rectangle(28.0, 22.0)),
            Self::Nose => Some(Collider::circle(9.0)),
            Self::Sensor => Some(Collider::circle(27.0)),
            Self::Pivot => None,
        }
    }

    pub(crate) fn collision_layers(self) -> CollisionLayers {
        let memberships = match self {
            Self::Hull | Self::Pivot => 0b0001,
            Self::Nose => 0b0010,
            Self::Sensor => 0b0100,
        };
        CollisionLayers::from_bits(memberships, LayerMask::ALL.0)
    }
}

/// Spawn an asymmetric compound player collider:
///
/// ```text
/// rigid-body root (no collider)
/// ├── hull collider
/// ├── transform-only pivot
/// │   └── nose collider
/// └── sensor collider
/// ```
pub(crate) fn spawn_player_parts(commands: &mut Commands, root: Entity, owner: PeerId) {
    let hull = PlayerPart {
        owner,
        kind: PlayerPartKind::Hull,
    };
    commands.spawn((
        ChildOf(root),
        hull,
        hull.kind.local_transform(),
        hull.kind.collider().unwrap(),
        ColliderDensity(0.2),
        Restitution::new(0.3),
        hull.kind.collision_layers(),
        Name::from("PlayerHullCollider"),
    ));

    let pivot = PlayerPart {
        owner,
        kind: PlayerPartKind::Pivot,
    };
    let pivot_entity = commands
        .spawn((
            ChildOf(root),
            pivot,
            pivot.kind.local_transform(),
            Name::from("PlayerColliderPivot"),
        ))
        .id();

    let nose = PlayerPart {
        owner,
        kind: PlayerPartKind::Nose,
    };
    commands.spawn((
        ChildOf(pivot_entity),
        nose,
        nose.kind.local_transform(),
        nose.kind.collider().unwrap(),
        ColliderDensity(0.08),
        Restitution::new(0.55),
        nose.kind.collision_layers(),
        Name::from("PlayerNoseCollider"),
    ));

    let sensor = PlayerPart {
        owner,
        kind: PlayerPartKind::Sensor,
    };
    commands.spawn((
        ChildOf(root),
        sensor,
        sensor.kind.local_transform(),
        sensor.kind.collider().unwrap(),
        Sensor,
        CollisionEventsEnabled,
        sensor.kind.collision_layers(),
        Name::from("PlayerSensorCollider"),
    ));
}

/// Install local transforms and predicted-only Avian collider components from a blueprint.
pub(crate) fn materialize_player_part(
    commands: &mut Commands,
    entity: Entity,
    part: PlayerPart,
    with_physics: bool,
    body: Option<Entity>,
) {
    let mut entity_commands = commands.entity(entity);
    entity_commands.insert(part.kind.local_transform());
    if !with_physics {
        return;
    }
    let Some(collider) = part.kind.collider() else {
        return;
    };
    entity_commands.insert((collider, part.kind.collision_layers()));
    if let Some(body) = body {
        entity_commands.insert(ColliderOf { body });
    }
    match part.kind {
        PlayerPartKind::Hull => {
            entity_commands.insert((ColliderDensity(0.2), Restitution::new(0.3)));
        }
        PlayerPartKind::Nose => {
            entity_commands.insert((ColliderDensity(0.08), Restitution::new(0.55)));
        }
        PlayerPartKind::Sensor => {
            entity_commands.insert((Sensor, CollisionEventsEnabled));
        }
        PlayerPartKind::Pivot => {}
    }
}

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        // bundles
        app.add_systems(Startup, init);

        // physics
        app.add_plugins(lightyear::avian2d::plugin::LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position {
                sync_to_transform: false,
            },
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                // disable the position<>transform sync plugins as it is handled by lightyear_avian
                .disable::<PhysicsTransformPlugin>()
                .disable::<PhysicsInterpolationPlugin>(),
        )
        .insert_resource(Gravity(Vec2::ZERO));

        crate::debug::register_debug_systems(app);
    }
}

pub(crate) fn init(mut commands: Commands) {
    commands.spawn(WallBundle::new(
        Vec2::new(-WALL_SIZE, -WALL_SIZE),
        Vec2::new(-WALL_SIZE, WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(-WALL_SIZE, WALL_SIZE),
        Vec2::new(WALL_SIZE, WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(WALL_SIZE, WALL_SIZE),
        Vec2::new(WALL_SIZE, -WALL_SIZE),
        Color::WHITE,
    ));
    commands.spawn(WallBundle::new(
        Vec2::new(WALL_SIZE, -WALL_SIZE),
        Vec2::new(-WALL_SIZE, -WALL_SIZE),
        Color::WHITE,
    ));
}

// Generates a pseudo-random color from the peer id.
pub(crate) fn color_from_id(client_id: PeerId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

// Applies movement input to a player velocity.
pub(crate) fn shared_movement_behaviour(
    mut velocity: Mut<LinearVelocity>,
    action: &ActionState<PlayerActions>,
) {
    trace!(pressed = ?action.get_pressed(), "shared movement");
    const MOVE_SPEED: f32 = 10.0;
    if action.pressed(&PlayerActions::Up) {
        velocity.y += MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Down) {
        velocity.y -= MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Left) {
        velocity.x -= MOVE_SPEED;
    }
    if action.pressed(&PlayerActions::Right) {
        velocity.x += MOVE_SPEED;
    }
    *velocity = LinearVelocity(velocity.clamp_length_max(MAX_VELOCITY));
}

// Wall
#[derive(Bundle)]
pub(crate) struct WallBundle {
    color: ColorComponent,
    physics: PhysicsBundle,
    wall: Wall,
    name: Name,
}

#[derive(Component)]
pub(crate) struct Wall {
    pub(crate) start: Vec2,
    pub(crate) end: Vec2,
}

impl WallBundle {
    pub(crate) fn new(start: Vec2, end: Vec2, color: Color) -> Self {
        Self {
            color: ColorComponent(color),
            physics: PhysicsBundle {
                collider: Collider::segment(start, end),
                collider_density: ColliderDensity(1.0),
                rigid_body: RigidBody::Static,
                restitution: Restitution::new(0.0),
            },
            wall: Wall { start, end },
            name: Name::from("Wall"),
        }
    }
}
