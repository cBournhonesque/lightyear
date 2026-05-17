use crate::protocol::*;
use crate::shared::Wall;
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::prelude::*;
use lightyear::prediction::Predicted;
use lightyear::prelude::{InterpolationSystems, RollbackSystems};
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
                .after(InterpolationSystems::Interpolate)
                .after(RollbackSystems::VisualCorrection),
        );

        // add visual interpolation for Position and Rotation
        // (normally we would interpolate on Transform but here this is fine
        // since rendering is done via Gizmos that only depend on Position/Rotation)
        app.add_plugins(FrameInterpolationPlugin::<Position>::default());
        app.add_plugins(FrameInterpolationPlugin::<Rotation>::default());
        app.add_observer(add_frame_interpolation_components);
    }
}

/// Predicted entities get updated in FixedUpdate, so we want to smooth/interpolate
/// their components in PostUpdate
fn add_frame_interpolation_components(
    // We use Position because it's added by avian later, and when it's added
    // we know that Predicted is already present on the entity
    trigger: On<Add, Position>,
    query: Query<Entity, With<Predicted>>,
    mut commands: Commands,
) {
    if query.contains(trigger.entity) {
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
    walls: Query<(&Wall, &ColorComponent), (Without<BallMarker>, Without<PlayerId>)>,
) {
    for (position, rotation, color) in &players {
        info!("Draw player at position {position:?}");
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
