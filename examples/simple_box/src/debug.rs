use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;

use crate::protocol::{Inputs, PlayerId, PlayerPosition};

#[cfg(feature = "client")]
pub(crate) mod client {
    use super::*;

    pub(crate) fn mark_debug_player_entities(
        mut commands: Commands,
        query: Query<(Entity, Has<Predicted>, Has<Interpolated>), Added<PlayerId>>,
    ) {
        for (entity, predicted, interpolated) in query.iter() {
            if predicted || interpolated {
                let input_sample_points = [
                    DebugSamplePoint::FixedPreUpdate,
                    DebugSamplePoint::FixedUpdate,
                ];
                commands.entity(entity).insert(
                    LightyearDebug::component_at::<PlayerPosition>([
                        DebugSamplePoint::Update,
                        DebugSamplePoint::PostUpdate,
                    ])
                    .with_component_at::<Predicted>([
                        DebugSamplePoint::Update,
                        DebugSamplePoint::PostUpdate,
                    ])
                    .with_component_at::<Interpolated>([
                        DebugSamplePoint::Update,
                        DebugSamplePoint::PostUpdate,
                    ])
                    .with_component_at::<ActionState<Inputs>>(input_sample_points),
                );
            }
        }
    }
}

#[cfg(feature = "server")]
pub(crate) mod server {
    use super::*;

    pub(crate) fn mark_debug_player_entities(
        mut commands: Commands,
        query: Query<Entity, Added<PlayerId>>,
    ) {
        for entity in query.iter() {
            let input_sample_points = [
                DebugSamplePoint::FixedPreUpdate,
                DebugSamplePoint::FixedUpdate,
            ];
            let debug =
                LightyearDebug::component_at::<PlayerPosition>([DebugSamplePoint::FixedUpdate])
                    .with_component_at::<ActionState<Inputs>>(input_sample_points);
            commands.entity(entity).insert(debug);
        }
    }
}
