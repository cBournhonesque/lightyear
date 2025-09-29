use crate::protocol::WeaponType::Hitscan;
use crate::shared::{DespawnAfter, color_from_id};
use avian2d::position::{Position, Rotation};
use avian2d::prelude::{CollisionLayers, LinearVelocity, PhysicsLayer, RigidBody};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use lightyear::input::bei::prelude;
use lightyear::input::prelude::InputConfig;
use lightyear::prelude::Channel;
use lightyear::prelude::input::bei::InputAction;
use lightyear::prelude::input::bei::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

pub const BULLET_SIZE: f32 = 3.0;
pub const PLAYER_SIZE: f32 = 40.0;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Bot;

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

#[derive(Component, MapEntities, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct BulletMarker {
    #[entities]
    pub shooter: Entity,
}

// Inputs
#[derive(Component, Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
pub struct PlayerContext;

#[derive(Debug, InputAction)]
#[action_output(Vec2)]
pub struct MovePlayer;

#[derive(Debug, InputAction)]
#[action_output(Vec2)]
pub struct MoveCursor;

#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct Shoot;

#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct CycleWeapon;

#[derive(Component, Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
pub struct ClientContext;

#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct CycleProjectileMode;

#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct CycleReplicationMode;

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

    pub fn fire_rate(&self) -> f32 {
        match self {
            WeaponType::Hitscan => 5.0,
            WeaponType::LinearProjectile => 2.0,
            WeaponType::Shotgun => 1.0,
            WeaponType::PhysicsProjectile => 1.5,
            WeaponType::HomingMissile => 0.5,
        }
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub enum ProjectileReplicationMode {
    FullEntity,    // Spawn a new entity per projectile
    DirectionOnly, // Only initial direction replicated
    RingBuffer,    // Weapon component with ring buffer
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

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, Reflect)]
pub enum GameReplicationMode {
    // TODO: do we predict other entities shooting? or just their movement?
    //  maybe just their movement?
    AllPredicted, // Current mode: client predicts all entities, server hit detection with no lag comp. (favors the shootee)
    ClientPredictedNoComp, // Client predicted, enemies interpolated, no lag comp
    ClientPredictedLagComp, // Client predicted, enemies interpolated, with lag comp
    ClientSideHitDetection, // Client predicted, enemies interpolated, hits computed on client
    AllInterpolated, // Everything interpolated with delay
    OnlyInputsReplicated, // Everything predicted, only inputs replicated
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
            GameReplicationMode::ClientPredictedNoComp => {
                GameReplicationMode::ClientPredictedLagComp
            }
            GameReplicationMode::ClientPredictedLagComp => {
                GameReplicationMode::ClientSideHitDetection
            }
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
    pub color: ColorComponent,
    pub weapon_type: WeaponType,
    pub shooter: Entity,
    pub player_id: PeerId,
}

#[derive(MapEntities, Event, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HitDetected {
    #[entities]
    pub shooter: Entity,
    #[entities]
    pub target: Entity,
}

pub struct HitChannel;

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<(
            GameReplicationMode,
            ProjectileReplicationMode,
            Actions<PlayerMarker>,
            ActionOf<PlayerMarker>,
            ActionOfWrapper<PlayerContext>,
            BulletMarker,
            PlayerId,
            ColorComponent,
            Score,
        )>();

        // inputs
        app.add_plugins(InputPlugin::new(InputConfig::<PlayerContext> {
            // enable lag compensation; the input messages sent to the server will include the
            // interpolation delay of that client
            lag_compensation: true,
            // enable input rebroadcasting so clients can predict other players' actions
            rebroadcast_inputs: true,
            ..default()
        }));
        app.register_input_action::<MovePlayer>();
        app.register_input_action::<MoveCursor>();
        app.register_input_action::<Shoot>();
        app.register_input_action::<CycleWeapon>();

        app.add_plugins(InputPlugin::new(InputConfig::<ClientContext> {
            // we don't want these actions to be replayed when a rollback happens
            ignore_rollbacks: true,
            ..default()
        }));
        app.register_input_action::<CycleProjectileMode>();
        app.register_input_action::<CycleReplicationMode>();

        // channel
        app.add_channel::<HitChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);

        // messages
        app.register_event::<HitDetected>()
            .add_map_entities()
            .add_direction(NetworkDirection::ClientToServer);

        // components
        app.register_component::<Name>();
        app.register_component::<PlayerId>();
        app.register_component::<PlayerMarker>();

        app.register_component::<Position>()
            .add_prediction()
            .add_should_rollback(position_should_rollback)
            .add_interpolation(InterpolationMode::Full)
            .add_linear_interpolation_fn()
            .add_linear_correction_fn();

        app.register_component::<Rotation>()
            .add_prediction()
            .add_should_rollback(rotation_should_rollback)
            .add_interpolation(InterpolationMode::Full)
            .add_linear_interpolation_fn();
        // .add_linear_correction_fn();

        app.register_component::<LinearVelocity>()
            .add_prediction();

        app.register_component::<ColorComponent>();

        app.register_component::<Score>();

        // we replicate HitscanVisual for the AllInterpolation mode
        // make sure that we have an Interpolated HitscanVisual entity since we only render entities
        // that are interpolated or predicted
        app.register_component::<HitscanVisual>();
        app.register_component::<RigidBody>();

        app.register_component::<BulletMarker>()
            .add_map_entities();

        app.register_component::<Bot>();

        app.register_component::<Score>();

        app.register_component::<PredictedBot>();

        app.register_component::<InterpolatedBot>();

        // Register new weapon and projectile components
        app.register_component::<WeaponType>();

        app.register_component::<Weapon>()
            .add_prediction();

        app.register_component::<ProjectileReplicationMode>();

        app.register_component::<GameReplicationMode>();

        app.register_component::<PlayerRoom>();

        app.register_component::<PhysicsProjectile>()
            .add_prediction()
            .add_linear_interpolation();

        app.register_component::<HomingMissile>()
            .add_prediction()
            .add_linear_interpolation();

        app.register_component::<ShotgunPellet>();

        app.register_component::<ProjectileSpawn>();

        // Make sure that we rollback the DespawnAfter timer in deterministic replication mode
        app.add_rollback::<DespawnAfter>();
    }
}

fn position_should_rollback(this: &Position, that: &Position) -> bool {
    (this.0 - that.0).length() >= 0.01
}

fn rotation_should_rollback(this: &Rotation, that: &Rotation) -> bool {
    this.angle_between(*that) >= 0.01
}
