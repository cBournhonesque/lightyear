use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;

use crate::protocol::{Inputs, PlayerId, PlayerTrail};

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
                    .insert(LightyearDebug::component_at::<PlayerTrail>([
                        DebugSamplePoint::Update,
                    ]));
            }
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
            commands.entity(entity).insert(
                LightyearDebug::component_at::<PlayerTrail>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<ActionState<Inputs>>([DebugSamplePoint::FixedUpdate]),
            );
        }
    }
}
