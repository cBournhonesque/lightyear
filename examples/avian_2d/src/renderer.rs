use crate::protocol::*;
use crate::shared::{PlayerChildCollider, Wall};
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::prelude::*;
use lightyear::prelude::{InterpolationSystems, RollbackSystems};

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
                .after(RollbackSystems::VisualCorrection)
                // Child colliders are rendered from GlobalTransform, so drawing before
                // propagation would show their previous-frame pose next to the current root.
                .after(TransformSystems::Propagate),
        );
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// System that draws the player's boxes and cursors
pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<(&Position, &Rotation, &ColorComponent), With<PlayerId>>,
    player_children: Query<(&GlobalTransform, &ChildOf), With<PlayerChildCollider>>,
    balls: Query<(&Position, &ColorComponent), With<BallMarker>>,
    walls: Query<(&Wall, &ColorComponent), (Without<BallMarker>, Without<PlayerId>)>,
) {
    for (position, rotation, color) in &players {
        debug!("Draw player at position {position:?}");
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
    for (global, child_of) in &player_children {
        let Ok((_, _, color)) = players.get(child_of.parent()) else {
            continue;
        };
        let transform = global.compute_transform();
        let position = transform.translation.truncate();
        let rotation = Rot2::radians(2.0 * transform.rotation.z.atan2(transform.rotation.w));
        gizmos.rect_2d(
            Isometry2d::new(position, rotation),
            Vec2::splat(CHILD_CUBE_SIZE),
            color.0.with_alpha(0.7),
        );
    }
    for (position, color) in &balls {
        gizmos.circle_2d(Vec2::new(position.x, position.y), BALL_SIZE, color.0);
    }
    for (wall, color) in &walls {
        gizmos.line_2d(wall.start, wall.end, color.0);
    }
}
