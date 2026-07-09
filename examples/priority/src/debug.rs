use bevy::prelude::*;
use lightyear::prelude::*;

use crate::protocol::{PlayerId, Position, Shape};

#[cfg(feature = "client")]
pub(crate) mod client {
    use super::*;

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        query: Query<(Entity, Has<Predicted>, Has<Interpolated>), Added<PlayerId>>,
    ) {
        for (entity, predicted, interpolated) in &query {
            if predicted || interpolated {
                commands
                    .entity(entity)
                    .insert(LightyearDebug::component_at::<Position>([
                        DebugSamplePoint::Update,
                    ]));
            }
        }
    }

    pub(crate) fn mark_debug_shapes(mut commands: Commands, shapes: Query<Entity, Added<Shape>>) {
        for entity in &shapes {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<Shape>([DebugSamplePoint::Update]),
            );
        }
    }
}

#[cfg(feature = "server")]
pub(crate) mod server {
    use super::*;

    pub(crate) fn mark_debug_players(
        mut commands: Commands,
        query: Query<Entity, Added<PlayerId>>,
    ) {
        for entity in &query {
            commands
                .entity(entity)
                .insert(LightyearDebug::component_at::<Position>([
                    DebugSamplePoint::FixedUpdate,
                ]));
        }
    }

    pub(crate) fn mark_debug_shapes(mut commands: Commands, shapes: Query<Entity, Added<Shape>>) {
        for entity in &shapes {
            commands.entity(entity).insert(
                LightyearDebug::component_at::<Position>([DebugSamplePoint::Update])
                    .with_component_at::<Shape>([DebugSamplePoint::Update]),
            );
        }
    }
}
