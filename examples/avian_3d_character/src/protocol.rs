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

        app.register_component::<ColorComponent>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<Name>()
            .add_prediction(PredictionMode::Once);

        app.register_component::<CharacterMarker>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<ProjectileMarker>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<FloorMarker>()
            .add_prediction(PredictionMode::Once);

        app.register_component::<BlockMarker>()
            .add_prediction(PredictionMode::Once);

        // Fully replicated, but not visual, so no need for lerp/corrections:
        app.register_component::<LinearVelocity>()
            .add_prediction(PredictionMode::Full);

        app.register_component::<AngularVelocity>()
            .add_prediction(PredictionMode::Full);

        app.register_component::<ExternalForce>()
            .add_prediction(PredictionMode::Full);

        app.register_component::<ExternalImpulse>()
            .add_prediction(PredictionMode::Full);

        app.register_component::<ComputedMass>()
            .add_prediction(PredictionMode::Full);

        // Position and Rotation have a `correction_fn` set, which is used to smear rollback errors
        // over a few frames, just for the rendering part in postudpate.
        //
        // They also set `interpolation_fn` which is used by the VisualInterpolationPlugin to smooth
        // out rendering between fixedupdate ticks.
        app.register_component::<Position>()
            .add_prediction(PredictionMode::Full)
            .add_should_rollback(position_should_rollback)
            .add_linear_correction_fn()
            .add_interpolation(InterpolationMode::Full)
            .add_linear_interpolation_fn();

        app.register_component::<Rotation>()
            .add_prediction(PredictionMode::Full)
            .add_should_rollback(rotation_should_rollback)
            .add_linear_correction_fn()
            .add_interpolation(InterpolationMode::Full)
            .add_linear_interpolation_fn();
    }
}

fn position_should_rollback(this: &Position, that: &Position) -> bool {
    (this.0 - that.0).length() >= 0.01
}

fn rotation_should_rollback(this: &Rotation, that: &Rotation) -> bool {
    this.angle_between(that.0) >= 0.01
}
