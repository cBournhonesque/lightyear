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

/// Local marker for the floor, which is not replicated.
///
/// The floor is spawned by [`SharedPlugin`](crate::shared::SharedPlugin) on both
/// the client and the server, so there is nothing to send: each side gets a
/// static collider at the same place. Replicating it would only add a
/// confirmation round-trip before the local body can be simulated.
///
/// [`FloorMarker`] is what the renderer and the debug view key on.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct FloorMarker;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BlockMarker;

/// Replicated: present while a player carries the block, naming the holder.
///
/// Combined with the receiver-local [`Controlled`](lightyear::prelude::Controlled)
/// marker (present only on the carrier's client), this lets every client tell
/// apart "I carry it" (predict), "someone else carries it" (follow the
/// holder's timeline) and "free" (proximity decides). The holder is the
/// character entity, which replicates everywhere, so the reference maps
/// consistently on all clients (unlike the sender-side `ControlledBy` link,
/// which never crosses the wire).
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CarriedBy {
    /// Character entity carrying this block.
    #[entities]
    pub holder: Entity,
}

/// Replicated marker: sphere-shaped blocks.
///
/// Cubes ride frozen (kinematic teleport) while carried; spheres dangle from
/// the same carry pose on a velocity leash, so they keep swinging with some
/// bounce on every timeline.
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

        // app.component::<ComputedMass>().replicate().predict();
        // The LightyearAvianPlugin registers Avian's Position, Rotation,
        // LinearVelocity, and AngularVelocity networking rules.
    }
}
