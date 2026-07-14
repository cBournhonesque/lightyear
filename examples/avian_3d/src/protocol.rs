use crate::shared::color_from_id;
use avian3d::prelude::*;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::input::prelude::InputConfig;
use lightyear::prelude::input::leafwing;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

// Components

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CharacterMarker;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FloorMarker;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProjectileMarker;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BlockMarker;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect, Serialize, Deserialize)]
pub enum CharacterAction {
    Move,
    Jump,
    Shoot,
}

impl Actionlike for CharacterAction {
    fn input_control_kind(&self) -> InputControlKind {
        match self {
            Self::Move => InputControlKind::DualAxis,
            Self::Jump => InputControlKind::Button,
            Self::Shoot => InputControlKind::Button,
        }
    }
}

// Protocol
#[derive(Clone)] // Added Clone
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(leafwing::InputPlugin::<CharacterAction> {
            config: InputConfig::<CharacterAction> {
                rebroadcast_inputs: true,
                ..default()
            },
        });

        app.component::<ColorComponent>().replicate();

        app.component::<Name>().replicate();

        app.component::<CharacterMarker>().replicate();

        app.component::<ProjectileMarker>().replicate();

        app.component::<FloorMarker>().replicate();

        app.component::<BlockMarker>().replicate();

        // Fully replicated, but not visual, so no need for lerp/corrections:
        app.component::<LinearVelocity>()
            .replicate()
            .predict()
            .with_rollback_condition(linear_velocity_should_rollback);

        app.component::<AngularVelocity>()
            .replicate()
            .predict()
            .with_rollback_condition(angular_velocity_should_rollback);

        // app.component::<ComputedMass>().replicate().predict();

        // Position and Rotation use their interpolation rules to smear rollback errors
        // over a few frames, just for rendering in PostUpdate.
        //
        // FrameInterpolationPlugin reuses the same interpolation rules to smooth
        // rendering between fixed ticks on entities marked with FrameInterpolate.
        app.component::<Position>()
            .replicate()
            .predict()
            .with_rollback_condition(position_should_rollback)
            .add_linear_interpolation()
            .add_correction();

        app.component::<Rotation>()
            .replicate()
            .predict()
            .with_rollback_condition(rotation_should_rollback)
            .add_linear_interpolation()
            .add_correction();
    }
}

fn position_should_rollback(this: &Position, that: &Position) -> bool {
    (this.0 - that.0).length() >= 0.01
}

fn rotation_should_rollback(this: &Rotation, that: &Rotation) -> bool {
    this.angle_between(*that) >= 0.01
}

fn linear_velocity_should_rollback(this: &LinearVelocity, that: &LinearVelocity) -> bool {
    (this.0 - that.0).length() >= 0.01
}

fn angular_velocity_should_rollback(this: &AngularVelocity, that: &AngularVelocity) -> bool {
    (this.0 - that.0).length() >= 0.01
}
