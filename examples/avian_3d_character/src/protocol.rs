use avian3d::prelude::*;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;
use serde::{Deserialize, Serialize};

use crate::shared::color_from_id;
use lightyear::client::components::{ComponentSyncMode, LerpFn};
use lightyear::client::interpolation::LinearInterpolator;
use lightyear::prelude::client::{self};
use lightyear::prelude::server::{Replicate, SyncTarget};
use lightyear::prelude::*;
use lightyear::shared::input::InputConfig;
use lightyear::utils::avian3d::{position, rotation};
use lightyear::utils::bevy::TransformLinearInterpolation;
use tracing_subscriber::util::SubscriberInitExt;

// For prediction, we want everything entity that is predicted to be part of
// the same replication group This will make sure that they will be replicated
// in the same message and that all the entities in the group will always be
// consistent (= on the same tick)
pub const REPLICATION_GROUP: ReplicationGroup = ReplicationGroup::new_id(1);

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
pub(crate) struct ProtocolPlugin {
    pub(crate) predict_all: bool,
}

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LeafwingInputPlugin::<CharacterAction> {
            config: InputConfig::<CharacterAction> {
                rebroadcast_inputs: self.predict_all,
                ..default()
            },
        });

        app.register_component::<ColorComponent>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_component::<Name>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once);

        app.register_component::<CharacterMarker>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_component::<ProjectileMarker>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_component::<FloorMarker>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once);

        app.register_component::<BlockMarker>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once);

        // Fully replicated, but not visual, so no need for lerp/corrections:
        app.register_component::<LinearVelocity>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full);

        app.register_component::<AngularVelocity>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full);

        app.register_component::<ExternalForce>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full);

        app.register_component::<ExternalImpulse>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full);

        // Do not replicate Transform when we are replicating Position/Rotation!
        // See https://github.com/cBournhonesque/lightyear/discussions/941
        // app.register_component::<Transform>(ChannelDirection::ServerToClient)
        //     .add_prediction(ComponentSyncMode::Full);

        app.register_component::<ComputedMass>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full);

        // Position and Rotation have a `correction_fn` set, which is used to smear rollback errors
        // over a few frames, just for the rendering part in postudpate.
        //
        // They also set `interpolation_fn` which is used by the VisualInterpolationPlugin to smooth
        // out rendering between fixedupdate ticks.
        app.register_component::<Position>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation_fn(position::lerp)
            .add_interpolation(ComponentSyncMode::Full)
            .add_correction_fn(position::lerp);

        app.register_component::<Rotation>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Full)
            .add_interpolation_fn(rotation::lerp)
            .add_interpolation(ComponentSyncMode::Full)
            .add_correction_fn(rotation::lerp);

        // do not replicate Transform but make sure to register an interpolation function
        // for it so that we can do visual interpolation
        // (another option would be to replicate transform and not use Position/Rotation at all)
        app.add_interpolation::<Transform>(ComponentSyncMode::None);
        app.add_interpolation_fn::<Transform>(TransformLinearInterpolation::lerp);
    }
}
