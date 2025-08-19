use avian2d::position::{Position, Rotation};
use avian2d::prelude::RigidBody;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::input::prelude::InputConfig;
use lightyear::prelude::input::{leafwing, native};
use lightyear::prelude::Channel;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::shared::color_from_id;

pub const BULLET_SIZE: f32 = 3.0;
pub const PLAYER_SIZE: f32 = 40.0;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PredictedBot;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct InterpolatedBot;

// Components
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerMarker;

/// Number of bullet hits
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct Score(pub usize);

#[derive(Component, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BulletMarker;

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect)]
pub enum PlayerActions {
    Up,
    Down,
    Left,
    Right,
    Shoot,
    MoveCursor,
    CycleWeapon,
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq, Clone, Copy, Hash, Reflect)]
pub enum ExampleActions {
    CycleProjectileMode,
    CycleReplicationMode,
    #[default]
    None,
}

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub enum WeaponType {
    Hitscan,
    LinearProjectile,
    Shotgun,
    PhysicsProjectile,
    HomingMissile,
}

impl Default for WeaponType {
    fn default() -> Self {
        WeaponType::Hitscan
    }
}

impl WeaponType {
    pub fn next(&self) -> Self {
        match self {
            WeaponType::Hitscan => WeaponType::LinearProjectile,
            WeaponType::LinearProjectile => WeaponType::Shotgun,
            WeaponType::Shotgun => WeaponType::PhysicsProjectile,
            WeaponType::PhysicsProjectile => WeaponType::HomingMissile,
            WeaponType::HomingMissile => WeaponType::Hitscan,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            WeaponType::Hitscan => "Hitscan",
            WeaponType::LinearProjectile => "Linear Projectile",
            WeaponType::Shotgun => "Shotgun",
            WeaponType::PhysicsProjectile => "Physics Projectile",
            WeaponType::HomingMissile => "Homing Missile",
        }
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub enum ProjectileReplicationMode {
    FullEntity,      // Regular entity replication with updates
    DirectionOnly,   // Only initial direction replicated, client simulates
    RingBuffer,      // Weapon component with ring buffer
}

impl Default for ProjectileReplicationMode {
    fn default() -> Self {
        ProjectileReplicationMode::FullEntity
    }
}

impl ProjectileReplicationMode {
    pub fn next(&self) -> Self {
        match self {
            ProjectileReplicationMode::FullEntity => ProjectileReplicationMode::DirectionOnly,
            ProjectileReplicationMode::DirectionOnly => ProjectileReplicationMode::RingBuffer,
            ProjectileReplicationMode::RingBuffer => ProjectileReplicationMode::FullEntity,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            ProjectileReplicationMode::FullEntity => "Full Entity Replication",
            ProjectileReplicationMode::DirectionOnly => "Direction-Only Replication",
            ProjectileReplicationMode::RingBuffer => "Ring Buffer Replication",
        }
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub enum GameReplicationMode {
    AllPredicted,              // Current mode: all predicted, server hit detection
    ClientPredictedNoComp,     // Client predicted, enemies interpolated, no lag comp
    ClientPredictedLagComp,    // Client predicted, enemies interpolated, with lag comp
    ClientSideHitDetection,    // Hits computed on client
    AllInterpolated,           // Everything interpolated with delay
    OnlyInputsReplicated,      // Everything predicted, only inputs replicated
}

impl Default for GameReplicationMode {
    fn default() -> Self {
        GameReplicationMode::AllPredicted
    }
}

impl GameReplicationMode {
    pub fn next(&self) -> Self {
        match self {
            GameReplicationMode::AllPredicted => GameReplicationMode::ClientPredictedNoComp,
            GameReplicationMode::ClientPredictedNoComp => GameReplicationMode::ClientPredictedLagComp,
            GameReplicationMode::ClientPredictedLagComp => GameReplicationMode::ClientSideHitDetection,
            GameReplicationMode::ClientSideHitDetection => GameReplicationMode::AllInterpolated,
            GameReplicationMode::AllInterpolated => GameReplicationMode::OnlyInputsReplicated,
            GameReplicationMode::OnlyInputsReplicated => GameReplicationMode::AllPredicted,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            GameReplicationMode::AllPredicted => "All Predicted (Server Hit Detection)",
            GameReplicationMode::ClientPredictedNoComp => "Client Predicted (No Lag Comp)",
            GameReplicationMode::ClientPredictedLagComp => "Client Predicted (Lag Comp)",
            GameReplicationMode::ClientSideHitDetection => "Client-Side Hit Detection",
            GameReplicationMode::AllInterpolated => "All Interpolated",
            GameReplicationMode::OnlyInputsReplicated => "Only Inputs Replicated",
        }
    }

    pub fn room_id(&self) -> usize {
        match self {
            GameReplicationMode::AllPredicted => 0,
            GameReplicationMode::ClientPredictedNoComp => 1,
            GameReplicationMode::ClientPredictedLagComp => 2,
            GameReplicationMode::ClientSideHitDetection => 3,
            GameReplicationMode::AllInterpolated => 4,
            GameReplicationMode::OnlyInputsReplicated => 5,
        }
    }

    pub fn from_room_id(room_id: usize) -> Self {
        match room_id {
            0 => GameReplicationMode::AllPredicted,
            1 => GameReplicationMode::ClientPredictedNoComp,
            2 => GameReplicationMode::ClientPredictedLagComp,
            3 => GameReplicationMode::ClientSideHitDetection,
            4 => GameReplicationMode::AllInterpolated,
            5 => GameReplicationMode::OnlyInputsReplicated,
            _ => GameReplicationMode::AllPredicted, // Default fallback
        }
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Weapon {
    pub weapon_type: WeaponType,
    pub projectile_replication_mode: ProjectileReplicationMode,
    pub fire_rate: f32, // shots per second
    pub last_fire_tick: Option<Tick>,
    // Ring buffer for projectiles (used with RingBuffer replication mode)
    pub projectile_buffer: Vec<ProjectileSpawnInfo>,
    pub buffer_capacity: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct ProjectileSpawnInfo {
    pub spawn_tick: Tick,
    pub position: Vec2,
    pub direction: Vec2,
    pub weapon_type: WeaponType,
}

impl Default for Weapon {
    fn default() -> Self {
        Self {
            weapon_type: WeaponType::default(),
            projectile_replication_mode: ProjectileReplicationMode::default(),
            fire_rate: 2.0, // 2 shots per second by default
            last_fire_tick: None,
            projectile_buffer: Vec::new(),
            buffer_capacity: 100,
        }
    }
}


#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct PlayerRoom {
    pub room_id: usize,
}

impl Default for PlayerRoom {
    fn default() -> Self {
        Self { room_id: 0 }
    }
}

// Additional projectile-specific components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct HitscanVisual {
    pub start: Vec2,
    pub end: Vec2,
    pub lifetime: f32,
    pub max_lifetime: f32,
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PhysicsProjectile {
    pub bounce_count: u32,
    pub max_bounces: u32,
    pub deceleration: f32,
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct HomingMissile {
    pub target_entity: Option<Entity>,
    pub turn_speed: f32,
    pub acceleration: f32,
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct ShotgunPellet {
    pub pellet_index: u32,
    pub spread_angle: f32,
}

// Components for direction-only replication
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct ProjectileSpawn {
    pub spawn_tick: Tick,
    pub position: Vec2,
    pub direction: Vec2,
    pub speed: f32,
    pub weapon_type: WeaponType,
    pub player_id: PeerId,
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct ClientProjectile {
    pub start_position: Vec2,
    pub direction: Vec2,
    pub speed: f32,
    pub spawn_tick: Tick,
    pub weapon_type: WeaponType,
}

impl Actionlike for PlayerActions {
    // Record what kind of inputs make sense for each action.
    fn input_control_kind(&self) -> InputControlKind {
        match self {
            Self::MoveCursor => InputControlKind::DualAxis,
            _ => InputControlKind::Button,
        }
    }
}

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<(PlayerActions, ColorComponent)>();
        // inputs
        // Use new input plugin path and default config
        app.add_plugins(leafwing::InputPlugin::<PlayerActions> {
            config: InputConfig::<PlayerActions> {
                // enable lag compensation; the input messages sent to the server will include the
                // interpolation delay of that client
                lag_compensation: true,
                ..default()
            },
        });
        app.add_plugins(native::InputPlugin::<ExampleActions>::default());
        // components
        app.register_component::<Name>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);
        app.register_component::<PlayerId>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);
        app.register_component::<PlayerMarker>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<Position>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full)
            .add_linear_interpolation_fn()
            .add_linear_correction_fn();

        app.register_component::<Rotation>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full)
            .add_linear_interpolation_fn()
            .add_linear_correction_fn();

        app.register_component::<ColorComponent>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<Score>();

        app.register_component::<RigidBody>()
            .add_prediction(PredictionMode::Once);

        app.register_component::<BulletMarker>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<PredictedBot>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<InterpolatedBot>()
            .add_interpolation(InterpolationMode::Once);

        // Register new weapon and projectile components
        app.register_component::<WeaponType>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<Weapon>()
            .add_prediction(PredictionMode::Full);

        app.register_component::<ProjectileReplicationMode>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<GameReplicationMode>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<PlayerRoom>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<PhysicsProjectile>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full);

        app.register_component::<HomingMissile>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full);

        app.register_component::<ShotgunPellet>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<ProjectileSpawn>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<ClientProjectile>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full);
    }
}
