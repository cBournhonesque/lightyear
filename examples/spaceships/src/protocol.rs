use bevy::prelude::*;
use bevy::utils::Duration;
use bevy_xpbd_2d::prelude::*;
use derive_more::{Add, Mul};
use leafwing_input_manager::prelude::*;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;
use serde::{Deserialize, Serialize};

use lightyear::client::components::{ComponentSyncMode, LerpFn};
use lightyear::client::interpolation::LinearInterpolator;
use lightyear::prelude::client::{self, LeafwingInputConfig};
use lightyear::prelude::server::{Replicate, SyncTarget};
use lightyear::prelude::*;
use lightyear::utils::bevy_xpbd_2d::*;
use tracing_subscriber::util::SubscriberInitExt;

use crate::shared::{color_from_id, CollisionPayload};

pub const BALL_SIZE: f32 = 15.0;
pub const BULLET_SIZE: f32 = 1.5;
pub const SHIP_WIDTH: f32 = 19.0;
pub const SHIP_LENGTH: f32 = 32.0;

// For prediction, we want everything entity that is predicted to be part of the same replication group
// This will make sure that they will be replicated in the same message and that all the entities in the group
// will always be consistent (= on the same tick)
pub const REPLICATION_GROUP: ReplicationGroup = ReplicationGroup::new_id(1);

// Bullet
#[derive(Bundle)]
pub(crate) struct BulletBundle {
    position: Position,
    velocity: LinearVelocity,
    color: ColorComponent,
    marker: BulletMarker,
    lifetime: Lifetime,
    collision_payload: CollisionPayload,
}

impl BulletBundle {
    pub(crate) fn new(position: Vec2, velocity: Vec2, color: Color, current_tick: Tick) -> Self {
        Self {
            position: Position(position),
            velocity: LinearVelocity(velocity),
            color: ColorComponent(color),
            lifetime: Lifetime {
                origin_tick: current_tick,
                lifetime: FIXED_TIMESTEP_HZ as i16 * 2,
            },
            marker: BulletMarker,
            collision_payload: CollisionPayload,
        }
    }
}

// Ball
#[derive(Bundle)]
pub(crate) struct BallBundle {
    position: Position,
    color: ColorComponent,
    replicate: Replicate,
    marker: BallMarker,
    physics: PhysicsBundle,
    name: Name,
}

impl BallBundle {
    pub(crate) fn new(position: Vec2, color: Color) -> Self {
        let sync_target = SyncTarget {
            prediction: NetworkTarget::All,
            ..default()
        };
        let replicate = Replicate {
            sync: sync_target,
            group: REPLICATION_GROUP,
            ..default()
        };
        Self {
            position: Position(position),
            color: ColorComponent(color),
            replicate,
            physics: PhysicsBundle::ball(),
            marker: BallMarker,
            name: Name::new("Ball"),
        }
    }
}

#[derive(Bundle)]
pub(crate) struct PhysicsBundle {
    pub(crate) collider: Collider,
    pub(crate) collider_density: ColliderDensity,
    pub(crate) rigid_body: RigidBody,
    pub(crate) external_force: ExternalForce,
}

impl PhysicsBundle {
    pub(crate) fn bullet() -> Self {
        Self {
            collider: Collider::circle(BULLET_SIZE),
            collider_density: ColliderDensity(5.0),
            rigid_body: RigidBody::Dynamic,
            external_force: ExternalForce::default(),
        }
    }

    pub(crate) fn ball() -> Self {
        Self {
            collider: Collider::circle(BALL_SIZE),
            collider_density: ColliderDensity(0.5),
            rigid_body: RigidBody::Dynamic,
            external_force: ExternalForce::ZERO.with_persistence(false),
        }
    }

    pub(crate) fn player_ship() -> Self {
        // triangle ship, pointing up the screen
        let points = vec![
            Vec2::new(0.0, SHIP_LENGTH / 2.),
            Vec2::new(SHIP_WIDTH / 2., -SHIP_LENGTH / 2.),
            Vec2::new(-SHIP_WIDTH / 2., -SHIP_LENGTH / 2.),
        ];
        let collider = Collider::convex_hull(points).unwrap();
        // Note: due to a bug in older (?) versions of bevy_xpbd, using a triangle collider here
        // sometimes caused strange behaviour. Unsure if this is fixed now.
        // Also, counter-clockwise ordering of points was required for convex hull creation (?)
        Self {
            collider,
            collider_density: ColliderDensity(1.0),
            rigid_body: RigidBody::Dynamic,
            external_force: ExternalForce::ZERO.with_persistence(false),
        }
    }
}

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Player {
    pub client_id: ClientId,
    pub nickname: String,
    pub rtt: Duration,
    pub jitter: Duration,
}

impl Player {
    pub fn new(client_id: ClientId, nickname: String) -> Self {
        Self {
            client_id,
            nickname,
            rtt: Duration::ZERO,
            jitter: Duration::ZERO,
        }
    }
}

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BallMarker;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BulletMarker;

// to debounce shots - once you fire (on `last_fire_tick` you have to wait `cooldown` ticks before firing again)
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Weapon {
    pub(crate) last_fire_tick: Tick,
    pub(crate) cooldown: u16,
    pub(crate) bullet_speed: f32,
}

impl Weapon {
    pub(crate) fn new(cooldown: u16) -> Self {
        Self {
            last_fire_tick: Tick(0),
            bullet_speed: 500.0,
            cooldown,
        }
    }
}

// despawns `lifetime` ticks after `origin_tick`
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Lifetime {
    pub(crate) origin_tick: Tick,
    /// number of ticks to live for
    pub(crate) lifetime: i16,
}

// Channels

#[derive(Channel)]
pub struct Channel1;

// Messages

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum PlayerActions {
    Up,
    Down,
    Left,
    Right,
    Fire,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum AdminActions {
    SendMessage,
    Reset,
}

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // messages
        app.add_message::<Message1>(ChannelDirection::Bidirectional);
        // inputs
        app.add_plugins(LeafwingInputPlugin::<PlayerActions>::default());
        app.add_plugins(LeafwingInputPlugin::<AdminActions>::default());
        // components

        // Player is synced as Simple, because we periodically update rtt ping stats
        app.register_component::<Player>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Simple);

        app.register_component::<ColorComponent>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once);

        app.register_component::<Name>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once);

        app.register_component::<BallMarker>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once);

        app.register_component::<BulletMarker>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once);

        app.register_component::<Lifetime>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once);

        app.register_component::<Weapon>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full);

        app.register_component::<CollisionPayload>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once);

        // Velocity and position fully replicated

        app.register_component::<LinearVelocity>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full);

        app.register_component::<AngularVelocity>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full);

        // Position and Rotation have a `correction_fn` set, which is used to smear rollback errors
        // over a few frames, just for the rendering part in postudpate.
        //
        // They also set `interpolation_fn` which is used by the VisualInterpolationPlugin to smooth
        // out rendering between fixedupdate ticks.
        app.register_component::<Position>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation_fn(position::lerp)
            .add_correction_fn(position::lerp);

        app.register_component::<Rotation>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation_fn(rotation::lerp)
            .add_correction_fn(rotation::lerp);

        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
