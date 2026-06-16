use avian3d::prelude::*;
use bevy::prelude::*;
use bevy_ahoy::CharacterLook;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerMarker;

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerSlot(pub u64);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ColorComponent(pub Color);

#[derive(Component, Default)]
pub struct PlayerInput;

#[derive(Clone)]
pub struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.register_component::<Name>();
        app.register_component::<PlayerMarker>();
        app.register_component::<PlayerId>();
        app.register_component::<PlayerSlot>();
        app.register_component::<ColorComponent>();

        app.register_component::<Position>()
            .add_prediction()
            .add_should_rollback(position_should_rollback)
            .add_linear_correction_fn()
            .add_linear_interpolation();

        app.register_component::<Rotation>()
            .add_prediction()
            .add_should_rollback(rotation_should_rollback)
            .add_linear_correction_fn()
            .add_linear_interpolation();

        app.register_component::<LinearVelocity>()
            .add_prediction()
            .add_should_rollback(linear_velocity_should_rollback);

        app.register_component::<CharacterLook>()
            .add_prediction()
            .add_should_rollback(character_look_should_rollback);
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

fn character_look_should_rollback(this: &CharacterLook, that: &CharacterLook) -> bool {
    (this.yaw - that.yaw).abs() >= 0.001 || (this.pitch - that.pitch).abs() >= 0.001
}
