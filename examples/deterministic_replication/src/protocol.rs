use crate::shared::color_from_id;
use avian2d::prelude::*;
use bevy::prelude::*;
use bevy_replicon::prelude::AppRuleExt;
use leafwing_input_manager::prelude::*;
use lightyear::input::config::InputConfig;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::input::leafwing;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

pub const BALL_SIZE: f32 = 15.0;
pub const PLAYER_SIZE: f32 = 40.0;

#[derive(Bundle)]
pub(crate) struct PhysicsBundle {
    pub(crate) collider: Collider,
    pub(crate) collider_density: ColliderDensity,
    pub(crate) rigid_body: RigidBody,
    pub(crate) restitution: Restitution,
}

impl PhysicsBundle {
    pub(crate) fn ball() -> Self {
        Self {
            collider: Collider::circle(BALL_SIZE),
            collider_density: ColliderDensity(0.05),
            rigid_body: RigidBody::Dynamic,
            restitution: Restitution::new(0.5),
        }
    }

    pub(crate) fn player() -> Self {
        Self {
            collider: Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),
            collider_density: ColliderDensity(0.2),
            rigid_body: RigidBody::Dynamic,
            restitution: Restitution::new(0.3),
        }
    }
}

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub PeerId);

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BallMarker;

/// Replicated component set by the server to indicate the tick at which
/// physics should start. Both server and client add physics components
/// when `LocalTimeline::tick() >= PhysicsStartTick.0`.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PhysicsStartTick(pub Tick);

// Messages

#[derive(Event, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Ready;

// Channel

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Channel1;

// Inputs
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum PlayerActions {
    Up,
    Down,
    Left,
    Right,
}

// Protocol
#[derive(Clone)]
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // inputs
        app.add_plugins(leafwing::InputPlugin::<PlayerActions> {
            config: InputConfig {
                rebroadcast_inputs: true,
                ..default()
            },
        });

        // channel
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ClientToServer);

        // messages
        app.register_event::<Ready>()
            .add_direction(NetworkDirection::ClientToServer);

        // components
        app.register_component::<PlayerId>();
        app.register_component::<PhysicsStartTick>();
        // Physics components are replicated once (initial value on entity spawn)
        // so that late-joining clients get the correct starting state.
        // add_rollback registers PredictionHistory for rollback/checksums.
        // add_confirmed_write ensures the replicated value goes to
        // PredictionHistory as confirmed state (instead of overwriting the
        // component), so input-triggered rollbacks snap to the correct value.
        app.replicate_once::<Position>();
        app.add_rollback::<Position>()
            .add_confirmed_write()
            .add_custom_hash(lightyear_avian2d::types::position::hash)
            .register_linear_interpolation()
            .add_linear_correction_fn();

        app.replicate_once::<Rotation>();
        app.add_rollback::<Rotation>()
            .add_confirmed_write()
            .add_custom_hash(lightyear_avian2d::types::rotation::hash)
            .register_linear_interpolation()
            .add_linear_correction_fn();

        app.replicate_once::<LinearVelocity>();
        app.add_rollback::<LinearVelocity>().add_confirmed_write();

        app.replicate_once::<AngularVelocity>();
        app.add_rollback::<AngularVelocity>().add_confirmed_write();
    }
}
