use avian2d::prelude::*;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;
use serde::{Deserialize, Serialize};

use crate::shared::color_from_id;
use lightyear::input::config::InputConfig;
use lightyear::prelude::input::leafwing;
use lightyear::prelude::*;
use tracing_subscriber::util::SubscriberInitExt;

pub const BULLET_SIZE: f32 = 1.5;
pub const SHIP_WIDTH: f32 = 19.0;
pub const SHIP_LENGTH: f32 = 32.0;

#[derive(Bundle)]
pub(crate) struct PhysicsBundle {
    pub(crate) collider: Collider,
    pub(crate) collider_density: ColliderDensity,
    pub(crate) rigid_body: RigidBody,
}

impl PhysicsBundle {
    pub(crate) fn bullet() -> Self {
        Self {
            collider: Collider::circle(BULLET_SIZE),
            collider_density: ColliderDensity(5.0),
            rigid_body: RigidBody::Dynamic,
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
        // Note: due to a bug in older (?) versions of avian, using a triangle collider here
        // sometimes caused strange behaviour. Unsure if this is fixed now.
        // Also, counter-clockwise ordering of points was required for convex hull creation (?)
        Self {
            collider,
            collider_density: ColliderDensity(1.0),
            rigid_body: RigidBody::Dynamic,
        }
    }
}

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Player {
    pub client_id: PeerId,
    pub nickname: String,
    pub rtt: Duration,
    pub jitter: Duration,
}

impl Player {
    pub fn new(client_id: PeerId, nickname: String) -> Self {
        Self {
            client_id,
            nickname,
            rtt: Duration::ZERO,
            jitter: Duration::ZERO,
        }
    }
}

/// A shared system generates these events on server and client.
/// On the server, we use them to manipulate player scores;
/// On the clients, we just use them for visual effects.
#[derive(Message, Debug)]
pub struct BulletHitMessage {
    pub bullet_owner: PeerId,
    pub bullet_color: Color,
    /// if it struck a player, this is their PeerId:
    pub victim_client_id: Option<PeerId>,
    pub position: Vec2,
}

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BallMarker {
    pub radius: f32,
}

impl BallMarker {
    pub fn new(radius: f32) -> Self {
        Self { radius }
    }

    pub fn physics_bundle(&self) -> PhysicsBundle {
        PhysicsBundle {
            collider: Collider::circle(self.radius),
            collider_density: ColliderDensity(1.5),
            rigid_body: RigidBody::Dynamic,
        }
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BulletMarker {
    pub owner: PeerId,
}

impl BulletMarker {
    pub fn new(owner: PeerId) -> Self {
        Self { owner }
    }
}

// Limiting firing rate: once you fire on `last_fire_tick` you have to wait `cooldown` ticks before firing again.
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

// increases if you hit another player with a bullet, decreases if you get hit.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Score(pub i32);

// despawns `lifetime` ticks after `origin_tick`
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct BulletLifetime {
    pub(crate) origin_tick: Tick,
    /// number of ticks to live for
    pub(crate) lifetime: i16,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum PlayerActions {
    Up,
    Down,
    Left,
    Right,
    Fire,
}

pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(leafwing::InputPlugin::<PlayerActions> {
            config: InputConfig::<PlayerActions> {
                rebroadcast_inputs: true,
                ..default()
            },
        });

        app.register_component::<Player>();

        app.register_component::<ColorComponent>();

        app.register_component::<Name>();

        app.register_component::<BallMarker>();

        app.register_component::<BulletMarker>();

        app.register_component::<BulletLifetime>();

        app.register_component::<Score>();

        // Fully replicated, but not visual, so no need for lerp/corrections:
        app.register_component::<LinearVelocity>()
            .add_prediction()
            .add_should_rollback(linear_velocity_should_rollback);

        app.register_component::<AngularVelocity>()
            .add_prediction()
            .add_should_rollback(angular_velocity_should_rollback);

        app.register_component::<Weapon>().add_prediction();

        // Position and Rotation have a `correction_fn` set, which is used to smear rollback errors
        // over a few frames, just for the rendering part in postudpate.
        //
        // They also set `interpolation_fn` which is used by the VisualInterpolationPlugin to smooth
        // out rendering between fixedupdate ticks.
        app.register_component::<Position>()
            .add_prediction()
            .add_should_rollback(position_should_rollback)
            .add_linear_interpolation()
            .add_linear_correction_fn();

        app.register_component::<Rotation>()
            .add_prediction()
            .add_should_rollback(rotation_should_rollback)
            .add_linear_interpolation()
            .add_linear_correction_fn();

        app.register_type::<Confirmed<Position>>();
    }
}

fn position_should_rollback(this: &Position, that: &Position) -> bool {
    (this.0 - that.0).length() >= 0.0001
}

fn rotation_should_rollback(this: &Rotation, that: &Rotation) -> bool {
    this.angle_between(*that) >= 0.0001
}

fn linear_velocity_should_rollback(this: &LinearVelocity, that: &LinearVelocity) -> bool {
    (this.0 - that.0).length() >= 0.0001
}

fn angular_velocity_should_rollback(this: &AngularVelocity, that: &AngularVelocity) -> bool {
    (this.0 - that.0) >= 0.0001
}
