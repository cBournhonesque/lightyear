use core::ops::{Add, Mul};

use bevy::prelude::*;
use leafwing_input_manager::action_state::ActionState;
use leafwing_input_manager::input_map::InputMap;
use leafwing_input_manager::prelude::Actionlike;
use leafwing_input_manager::InputManagerBundle;
use serde::{Deserialize, Serialize};
use tracing::info;

use lightyear::prelude::client::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: Position,
    color: PlayerColor,
    // replicate: Replicate, // NOTE: replication is handled by the server plugin
    action_state: ActionState<Inputs>,
}

impl PlayerBundle {
    pub(crate) fn new(id: PeerId, position: Vec2) -> Self {
        // Generate pseudo random color from client id.
        let h = (((id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
        let s = 0.8;
        let l = 0.5;
        let color = Color::hsl(h, s, l);

        // let replicate = Replicate {
        //     sync: SyncTarget {
        //         prediction: NetworkTarget::Single(id),
        //         interpolation: NetworkTarget::AllExceptSingle(id),
        //     },
        //     controlled_by: ControlledBy {
        //         target: NetworkTarget::Single(id),
        //         ..default()
        //     },
        //     ..default()
        // };
        Self {
            id: PlayerId(id),
            position: Position(position),
            color: PlayerColor(color),
            // replicate,
            action_state: ActionState::default(),
        }
    }
    pub(crate) fn get_input_map() -> InputMap<Inputs> {
        InputMap::new([
            (Inputs::Right, KeyCode::ArrowRight),
            (Inputs::Right, KeyCode::KeyD),
            (Inputs::Left, KeyCode::ArrowLeft),
            (Inputs::Left, KeyCode::KeyA),
            (Inputs::Up, KeyCode::ArrowUp),
            (Inputs::Up, KeyCode::KeyW),
            (Inputs::Down, KeyCode::ArrowDown),
            (Inputs::Down, KeyCode::KeyS),
        ])
    }
}

// Components

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct Position(pub(crate) Vec2);

impl Ease for Position {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| Position(Vec2::lerp(start.0, end.0, t)))
    }
}

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

#[derive(Component, Deref, DerefMut)]
pub struct ShapeChangeTimer(pub(crate) Timer);

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub enum Shape {
    Circle,
    Triangle,
    Square,
}

// Channels

// Channels
pub struct Channel1;

// Messages

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Hash, Reflect, Clone, Copy, Actionlike)]
pub enum Inputs {
    Up,
    Down,
    Left,
    Right,
    Delete,
}

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // messages
        app.add_message::<Message1>()
            .add_direction(NetworkDirection::Bidirectional);
        // inputs
        app.add_plugins(input::leafwing::InputPlugin::<Inputs>::default());
        // components
        app.register_component::<PlayerId>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<Position>()
            .add_prediction(PredictionMode::Full)
            .add_interpolation(InterpolationMode::Full)
            .add_linear_interpolation_fn();

        app.register_component::<PlayerColor>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);

        app.register_component::<Shape>()
            .add_prediction(PredictionMode::Once)
            .add_interpolation(InterpolationMode::Once);
        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        });
    }
}
