use bevy::prelude::*;
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

#[derive(Component, Clone, Debug, PartialEq)]
pub struct FloorMarker;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BlockMarker;

/// Replicated: present while a player carries the block, naming the holder.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CarriedBy {
    /// Character entity carrying this block.
    #[entities]
    pub holder: Entity,
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SphereMarker;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect, Serialize, Deserialize)]
pub enum CharacterAction {
    Move,
    Jump,
    Pickup,
}

impl Actionlike for CharacterAction {
    fn input_control_kind(&self) -> InputControlKind {
        match self {
            Self::Move => InputControlKind::DualAxis,
            Self::Jump | Self::Pickup => InputControlKind::Button,
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
                // Every client predicts every character, so they need the remote inputs too.
                rebroadcast_inputs: true,
                ..default()
            },
        });

        app.component::<ColorComponent>().replicate();

        app.component::<Name>().replicate();

        app.component::<CharacterMarker>().replicate();

        app.component::<BlockMarker>().replicate();

        app.component::<CarriedBy>().replicate();

        app.component::<SphereMarker>().replicate();

        // The LightyearAvianPlugin registers Avian's Position, Rotation,
        // LinearVelocity, and AngularVelocity networking rules.
    }
}
