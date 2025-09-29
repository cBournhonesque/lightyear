//! This file contains the shared [`Protocol`] that defines the messages that can be sent between the client and server.
//!
//! You will need to define the [`Components`], [`Messages`] and [`Inputs`] that make up the protocol.
//! You can use the `#[protocol]` attribute to specify additional behaviour:
//! - how entities contained in the message should be mapped from the remote world to the local world
//! - how the component should be synchronized between the `Confirmed` entity and the `Predicted`/`Interpolated` entity
use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use bevy::math::Curve;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use lightyear::input::native::plugin::InputPlugin;
use lightyear::prelude::*;

// Player
#[derive(Bundle)]
pub(crate) struct PlayerBundle {
    id: PlayerId,
    position: PlayerPosition,
    color: PlayerColor,
}

impl PlayerBundle {
    pub(crate) fn new(id: PeerId, position: Vec2) -> Self {
        // Generate pseudo random color from client id.
        let h = (((id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
        let s = 0.8;
        let l = 0.5;
        let color = Color::hsl(h, s, l);
        Self {
            id: PlayerId(id),
            position: PlayerPosition(position),
            color: PlayerColor(color),
        }
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default, Reflect)]
pub struct Lobbies {
    pub lobbies: Vec<Lobby>,
}

impl Lobbies {
    /// Return true if there is an empty lobby available for players to join
    pub(crate) fn has_empty_lobby(&self) -> bool {
        if self.lobbies.is_empty() {
            return false;
        }
        self.lobbies.iter().any(|lobby| lobby.players.is_empty())
    }

    /// Remove a client from a lobby
    pub(crate) fn remove_client(&mut self, client_id: PeerId, commands: &mut Commands) {
        let mut removed_lobby = None;
        for (lobby_id, lobby) in self.lobbies.iter_mut().enumerate() {
            if let Some(index) = lobby.players.iter().position(|id| *id == client_id) {
                lobby.players.remove(index);
                if lobby.players.is_empty() {
                    removed_lobby = Some(lobby_id);
                    commands.entity(lobby.room).despawn();
                }
            }
        }
        if let Some(lobby_id) = removed_lobby {
            self.lobbies.remove(lobby_id);
            // always make sure that there is an empty lobby for players to join
            if !self.has_empty_lobby() {
                let room = commands.spawn(Room::default()).id();
                self.lobbies.push(Lobby::new(room));
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct Lobby {
    pub players: Vec<PeerId>,
    /// Which client is selected to be the host for the next game (if None, the server will be the host)
    pub host: Option<PeerId>,
    pub room: Entity,
    /// If true, the lobby is in game. If not, it is still in lobby mode
    pub in_game: bool,
}

impl Lobby {
    pub(crate) fn new(room: Entity) -> Self {
        Self {
            players: vec![],
            host: None,
            room,
            in_game: false,
        }
    }
}

// Components

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub struct PlayerId(pub PeerId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut, Reflect)]
pub struct PlayerPosition(pub Vec2);

impl Ease for PlayerPosition {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| {
            PlayerPosition(Vec2::lerp(start.0, end.0, t))
        })
    }
}

#[derive(Component, Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct PlayerColor(pub(crate) Color);

// Channels
pub struct Channel1;

// Messages

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StartGame {
    pub(crate) lobby_id: usize,
    pub(crate) host: Option<PeerId>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ExitLobby {
    pub(crate) lobby_id: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct JoinLobby {
    pub(crate) lobby_id: usize,
}

// Inputs

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Reflect)]
pub struct Direction {
    pub(crate) up: bool,
    pub(crate) down: bool,
    pub(crate) left: bool,
    pub(crate) right: bool,
}

impl Direction {
    pub(crate) fn is_none(&self) -> bool {
        !self.up && !self.down && !self.left && !self.right
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
pub enum Inputs {
    Direction(Direction),
}

impl Default for Inputs {
    fn default() -> Self {
        Inputs::Direction(Direction {
            up: false,
            down: false,
            left: false,
            right: false,
        })
    }
}

impl MapEntities for Inputs {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {}
}

// Protocol
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<(PlayerPosition, PlayerId, Lobbies)>();
        // messages
        app.register_message::<StartGame>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_message::<JoinLobby>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<ExitLobby>()
            .add_direction(NetworkDirection::ClientToServer);
        // inputs
        app.add_plugins(InputPlugin::<Inputs>::default());
        // components
        app.register_component::<Name>();
        app.register_component::<PlayerId>();

        app.register_component::<PlayerPosition>()
            .add_prediction()
            .add_linear_interpolation();

        app.register_component::<PlayerColor>();

        app.register_component::<Lobbies>();

        // channels
        app.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);
    }
}
