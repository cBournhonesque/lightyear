use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;

use crate::protocol::{Inputs, Lobbies, PlayerId, PlayerPosition};

#[cfg(feature = "client")]
pub(crate) mod client {
    use super::*;

    pub(crate) fn mark_debug_lobbies(
        mut commands: Commands,
        lobbies: Query<Entity, Added<Lobbies>>,
    ) {
        for entity in &lobbies {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Lobbies>([
                    DebugSamplePoint::Update,
                ]));
        }
    }

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        players: Query<(Entity, Has<Predicted>, Has<Interpolated>), Added<PlayerId>>,
    ) {
        for (entity, predicted, interpolated) in &players {
            if predicted || interpolated {
                commands.entity(entity).insert(
                    LightyearDebug::component_at::<PlayerPosition>([DebugSamplePoint::Update])
                        .with_component_at::<ActionState<Inputs>>([DebugSamplePoint::Update])
                        .with_component_at::<PlayerId>([DebugSamplePoint::Update]),
                );
            }
        }
    }
}

#[cfg(any(feature = "client", feature = "server"))]
pub(crate) mod server {
    use super::*;

    pub(crate) fn mark_debug_lobbies(
        mut commands: Commands,
        lobbies: Query<Entity, Added<Lobbies>>,
    ) {
        for entity in &lobbies {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Lobbies>([
                    DebugSamplePoint::Update,
                ]));
        }
    }

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        players: Query<Entity, Added<PlayerId>>,
    ) {
        for entity in &players {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<PlayerPosition>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<ActionState<Inputs>>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<PlayerId>([DebugSamplePoint::FixedUpdate]),
            );
        }
    }
}
