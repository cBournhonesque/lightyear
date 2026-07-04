use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::input::prelude::InputConfig;
use lightyear::prelude::input::leafwing;
use lightyear::prelude::Channel;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::shared::color_from_id;

const ROLLBACK_ROTATION_EPSILON: f32 = 0.0001;

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

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct BulletMarker {
    pub shooter: PeerId,
    pub fire_tick: Tick,
    pub salt: u64,
    pub prespawn_hash: u64,
}

impl BulletMarker {
    pub fn new(shooter: PeerId, fire_tick: Tick, salt: u64, prespawn_hash: u64) -> Self {
        Self {
            shooter,
            fire_tick,
            salt,
            prespawn_hash,
        }
    }
}

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect)]
pub enum PlayerActions {
    Up,
    Down,
    Left,
    Right,
    Shoot,
    MoveCursor,
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
        // components
        app.component::<Name>().replicate();
        app.component::<PlayerId>().replicate();
        app.component::<PlayerMarker>().replicate();

        app.component::<Position>()
            .replicate()
            .predict()
            .add_linear_interpolation()
            // we enable correction without applying Correction on Position.
            // Instead we will apply Correction/FrameInterpolation on Transform directly.
            .enable_correction();

        app.component::<Rotation>()
            .replicate()
            .predict()
            .with_rollback_condition(rotation_should_rollback)
            .enable_correction()
            .add_linear_interpolation();

        // Bullet motion is simulated by Avian from LinearVelocity. Predicted bullets need the same
        // velocity in rollback history as the server entity, otherwise replay re-simulates from stale
        // or missing physics state and repeatedly corrects bullet positions.
        app.component::<LinearVelocity>().replicate().predict();

        app.component::<ColorComponent>().replicate();

        app.component::<Score>().replicate();

        app.component::<RigidBody>().replicate();

        app.component::<BulletMarker>()
            .replicate()
            .add_custom_interpolation();

        app.component::<PredictedBot>().replicate();

        app.component::<InterpolatedBot>().replicate();
    }
}

fn rotation_should_rollback(confirmed: &Rotation, predicted: &Rotation) -> bool {
    (confirmed.cos - predicted.cos).abs() > ROLLBACK_ROTATION_EPSILON
        || (confirmed.sin - predicted.sin).abs() > ROLLBACK_ROTATION_EPSILON
}
