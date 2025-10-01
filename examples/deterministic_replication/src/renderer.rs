use crate::protocol::*;
use crate::shared::Wall;
use avian2d::prelude::{Position, Rotation};
use bevy::prelude::*;
use lightyear::prediction::Predicted;
use lightyear::prediction::rollback::DeterministicPredicted;
use lightyear::prelude::{Client, Connected, EventSender, InterpolationSet, RollbackSet};
use lightyear_frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        // draw after interpolation is done
        app.add_systems(
            PostUpdate,
            draw_elements
                .after(InterpolationSet::Interpolate)
                .after(RollbackSet::VisualCorrection),
        );

        // add visual interpolation for Position and Rotation
        // (normally we would interpolate on Transform but here this is fine
        // since rendering is done via Gizmos that only depend on Position/Rotation)
        app.add_plugins(FrameInterpolationPlugin::<Position>::default());
        app.add_plugins(FrameInterpolationPlugin::<Rotation>::default());
        app.add_observer(add_visual_interpolation_components);
    }
}

fn add_visual_interpolation_components(
    trigger: On<Add, Position>,
    predicted: Query<(), With<DeterministicPredicted>>,
    mut commands: Commands,
) {
    if let Ok(()) = predicted.get(trigger.entity) {
        commands.entity(trigger.entity).insert((
            FrameInterpolate::<Position>::default(),
            FrameInterpolate::<Rotation>::default(),
        ));
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// System that draws the player's boxes and cursors
pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<(&Position, &Rotation, &ColorComponent), With<PlayerId>>,
    balls: Query<(&Position, &ColorComponent), With<BallMarker>>,
    walls: Query<(&Wall, &ColorComponent), Without<PlayerId>>,
) {
    for (position, rotation, color) in &players {
        gizmos.rect_2d(
            Isometry2d {
                rotation: Rot2 {
                    sin: rotation.sin,
                    cos: rotation.cos,
                },
                translation: Vec2::new(position.x, position.y),
            },
            Vec2::ONE * PLAYER_SIZE,
            color.0,
        );
    }
    for (position, color) in &balls {
        gizmos.circle_2d(Vec2::new(position.x, position.y), BALL_SIZE, color.0);
    }
    for (wall, color) in &walls {
        gizmos.line_2d(wall.start, wall.end, color.0);
    }
}
