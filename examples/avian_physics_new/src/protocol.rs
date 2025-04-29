use avian2d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use serde::{Deserialize, Serialize};

use crate::shared::color_from_id;
// Use preludes
use lightyear::prelude::client::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::utils::avian2d::*; // Keep avian utils

pub const BALL_SIZE: f32 = 15.0;
pub const PLAYER_SIZE: f32 = 40.0;

// For prediction, we want everything entity that is predicted to be part of the same replication group
// This will make sure that they will be replicated in the same message and that all the entities in the group
// will always be consistent (= on the same tick)
pub const REPLICATION_GROUP: ReplicationGroup = ReplicationGroup::new_id(1);

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: Position,
    color: ColorComponent,
    replicate: ReplicateToServer,
    physics: PhysicsBundle,
    inputs: InputManagerBundle<PlayerActions>,
    // IMPORTANT: this lets the server know that the entity is pre-predicted
    // when the server replicates this entity; we will get a Confirmed entity which will use this entity
    // as the Predicted version
    pre_predicted: PrePredicted,
    name: Name,
}

impl PlayerBundle {
    // Updated to use PeerId
    pub(crate) fn new(id: PeerId, position: Vec2, input_map: InputMap<PlayerActions>) -> Self {
        let color = color_from_id(id);
        Self {
            id: PlayerId(id), // Store PeerId
            position: Position(position),
            color: ColorComponent(color),
            // ReplicateToServer is handled implicitly by client predicting the entity
            // replicate: ReplicateToServer::default(),
            physics: PhysicsBundle::player(),
            inputs: InputManagerBundle::<PlayerActions> {
                action_state: ActionState::default(),
                input_map,
            },
            pre_predicted: PrePredicted::default(),
            name: Name::from("Player"),
        }
    }
}

// Ball
#[derive(Bundle)]
pub(crate) struct BallBundle {
    position: Position,
    color: ColorComponent,
    // Use new replication components directly
    replicate: Replicate,
    prediction_target: Option<PredictionTarget>,
    interpolation_target: Option<InterpolationTarget>,
    marker: BallMarker,
    physics: PhysicsBundle,
    name: Name,
}

impl BallBundle {
    pub(crate) fn new(position: Vec2, color: Color, predicted: bool) -> Self {
        let replicate = Replicate::to_clients(NetworkTarget::All); // Default replicate to all
        let (prediction_target, interpolation_target, group) = if predicted {
            (
                Some(PredictionTarget::to_clients(NetworkTarget::All)),
                None, // No interpolation if predicted
                REPLICATION_GROUP, // Use prediction group
            )
        } else {
            (
                None, // No prediction if not predicted
                Some(InterpolationTarget::to_clients(NetworkTarget::All)),
                ReplicationGroup::default(), // Default group if not predicted
            )
        };

        Self {
            position: Position(position),
            color: ColorComponent(color),
            replicate: replicate.set_group(group), // Set group on replicate
            prediction_target,
            interpolation_target,
            physics: PhysicsBundle::ball(),
            marker: BallMarker,
            name: Name::from("Ball"),
        }
    }
}

#[derive(Bundle)]
pub(crate) struct PhysicsBundle {
    pub(crate) collider: Collider,
    pub(crate) collider_density: ColliderDensity,
    pub(crate) rigid_body: RigidBody,
}

impl PhysicsBundle {
    pub(crate) fn ball() -> Self {
        Self {
            collider: Collider::circle(BALL_SIZE),
            collider_density: ColliderDensity(0.05),
            rigid_body: RigidBody::Dynamic,
        }
    }

    pub(crate) fn player() -> Self {
        Self {
            collider: Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),
            collider_density: ColliderDensity(0.2),
            rigid_body: RigidBody::Dynamic,
        }
    }
}

// Components
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub PeerId); // Use PeerId

// Resource to store the client's PeerId
#[derive(Resource, Debug, Clone, Copy)]
pub struct LocalPlayerId(pub PeerId);


#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct ColorComponent(pub(crate) Color);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BallMarker;

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
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum AdminActions {
    SendMessage,
    Reset,
}

// Protocol
#[derive(Clone)] // Added Clone
pub(crate) struct ProtocolPlugin;
// { // Removed predict_all field
//     pub(crate) predict_all: bool,
// }

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // messages
        app.register_message::<Message1>(ChannelDirection::Bidirectional);
        // inputs
        // Use new input plugin path and default config
        app.add_plugins(input::leafwing::InputPlugin::<PlayerActions>::default());
        // app.add_plugins(LeafwingInputPlugin::<PlayerActions> {
        //     config: InputConfig::<PlayerActions> {
        //         rebroadcast_inputs: self.predict_all, // Removed config
        //         ..default()
        //     },
        // });
        app.add_plugins(input::leafwing::InputPlugin::<AdminActions>::default());
        // components
        // Use PredictionMode and InterpolationMode
        app.register_component::<PlayerId>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<ColorComponent>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<BallMarker>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<Position>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full)
            .add_interpolation_fn(position::lerp)
            .add_correction_fn(position::lerp);

        app.register_component::<Rotation>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full)
            .add_interpolation_fn(rotation::lerp)
            .add_correction_fn(rotation::lerp);

        // NOTE: interpolation/correction is only needed for components that are visually displayed!
        // we still need prediction to be able to correctly predict the physics on the client
        app.register_component::<LinearVelocity>()
            .add_prediction(PredictionMode::Full);

        app.register_component::<AngularVelocity>()
            .add_prediction(PredictionMode::Full);

        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
