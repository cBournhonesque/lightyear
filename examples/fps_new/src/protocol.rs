use avian2d::prelude::RigidBody;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use serde::{Deserialize, Serialize};

// Use preludes
use lightyear::prelude::client::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::prelude::Channel; // Explicitly import Channel trait
// Removed unused imports
// use lightyear::shared::input::InputConfig;
// use lightyear::shared::replication::components::ReplicationGroupIdBuilder;
use lightyear::utils::bevy::*; // Keep bevy utils

use crate::shared::color_from_id;

pub const BULLET_SIZE: f32 = 3.0;
pub const PLAYER_SIZE: f32 = 40.0;

// For prediction, we want everything entity that is predicted to be part of the same replication group
// This will make sure that they will be replicated in the same message and that all the entities in the group
// will always be consistent (= on the same tick)
pub const REPLICATION_GROUP: ReplicationGroup = ReplicationGroup::new_id(1);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PredictedBot;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct InterpolatedBot;

// Components
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub PeerId); // Use PeerId

/// Number of bullet hits
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct Score(pub usize);

#[derive(Component, Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BulletMarker;

// Channels

#[derive(Channel)]
pub struct Channel1;

// Removed manual impl Channel block
// impl Channel for Channel1 {
//     fn name(&self) -> &'static str {
//         "Channel1"
//     }
// }

// Messages

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

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
        // messages
        app.register_message::<Message1>(ChannelDirection::Bidirectional);
        // inputs
        // Use new input plugin path and default config
        app.add_plugins(input::leafwing::InputPlugin::<PlayerActions>::default());
        // app.add_plugins(LeafwingInputPlugin::<PlayerActions> {
        //     config: InputConfig::<PlayerActions> {
        //         // enable lag compensation; the input messages sent to the server will include the
        //         // interpolation delay of that client
        //         lag_compensation: true, // Assuming lag compensation is handled elsewhere or default
        //         ..default()
        //     },
        // });
        // components
        // Use PredictionMode and InterpolationMode
        app.register_component::<Name>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);
        app.register_component::<PlayerId>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<Transform>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full)
            .add_interpolation_fn(TransformLinearInterpolation::lerp);

        app.register_component::<ColorComponent>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        // Score component doesn't need prediction/interpolation by default
        app.register_component::<Score>();

        // RigidBody might only need prediction if physics runs client-side?
        // Assuming Once is okay for now.
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

        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
