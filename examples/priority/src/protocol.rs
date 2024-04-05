use std::ops::Mul;

use bevy::prelude::*;
use derive_more::{Add, Mul};
use leafwing_input_manager::action_state::ActionState;
use leafwing_input_manager::input_map::InputMap;
use leafwing_input_manager::prelude::Actionlike;
use leafwing_input_manager::InputManagerBundle;
use serde::{Deserialize, Serialize};
use tracing::info;

use lightyear::prelude::*;

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: Position,
    color: PlayerColor,
    replicate: Replicate,
    action_state: ActionState<Inputs>,
}

impl PlayerBundle {
    pub(crate) fn new(id: ClientId, position: Vec2) -> Self {
        // Generate pseudo random color from client id.
        let h = (((id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
        let s = 0.8;
        let l = 0.5;
        let color = Color::hsl(h, s, l);

        let mut replicate = Replicate {
            prediction_target: NetworkTarget::Single(id),
            interpolation_target: NetworkTarget::AllExceptSingle(id),
            ..default()
        };
        // We don't want to replicate the ActionState to the original client, since they are updating it with
        // their own inputs (if you replicate it to the original client, it will be added on the Confirmed entity,
        // which will keep syncing it to the Predicted entity because the ActionState gets updated every tick)!
        replicate.add_target::<ActionState<Inputs>>(NetworkTarget::AllExceptSingle(id));
        Self {
            id: PlayerId(id),
            position: Position(position),
            color: PlayerColor(color),
            replicate,
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
            (Inputs::Delete, KeyCode::Backspace),
            (Inputs::Spawn, KeyCode::Space),
            (Inputs::Message, KeyCode::KeyM),
        ])
    }
}

// Components

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(pub ClientId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Add, Mul)]
pub struct Position(pub(crate) Vec2);

impl Mul<f32> for &Position {
    type Output = Position;

    fn mul(self, rhs: f32) -> Self::Output {
        Position(self.0 * rhs)
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

#[component_protocol(protocol = "MyProtocol")]
pub enum Components {
    #[protocol(sync(mode = "once"))]
    PlayerId(PlayerId),
    #[protocol(sync(mode = "full"))]
    PlayerPosition(Position),
    #[protocol(sync(mode = "once"))]
    PlayerColor(PlayerColor),
    Shape(Shape),
}

// Channels

#[derive(Channel)]
pub struct Channel1;

// Messages

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(pub usize);

#[message_protocol(protocol = "MyProtocol")]
pub enum Messages {
    Message1(Message1),
}

// Inputs

#[derive(
    Serialize, Deserialize, Debug, Default, PartialEq, Eq, Hash, Reflect, Clone, Copy, Actionlike,
)]
pub enum Inputs {
    Up,
    Down,
    Left,
    Right,
    Delete,
    Spawn,
    Message,
    #[default]
    None,
}

impl LeafwingUserAction for Inputs {}

// Protocol

protocolize! {
    Self = MyProtocol,
    Message = Messages,
    Component = Components,
    LeafwingInput1 = Inputs,
}

pub(crate) fn protocol() -> MyProtocol {
    let mut protocol = MyProtocol::default();
    protocol.add_channel::<Channel1>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        ..default()
    });
    protocol
}
