use bevy::prelude::*;
use bevy::render::RenderPlugin;

use crate::protocol::*;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(Update, (draw_trails, draw_boxes));
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// System that draws the boxes of the player positions.
/// The components should be replicated from the server to the client
pub(crate) fn draw_boxes(mut gizmos: Gizmos, mut players: Query<(&PlayerPosition, &PlayerColor)>) {
    for (position, color) in &mut players {
        gizmos.rect_2d(
            Isometry2d::from_translation(position.0),
            Vec2::ONE * 50.0,
            color.0,
        );
    }
}

pub(crate) fn draw_trails(mut gizmos: Gizmos, trails: Query<(&PlayerTrail, &PlayerColor)>) {
    for (trail, color) in &trails {
        let len = trail.0.len();
        if len == 0 {
            continue;
        }
        for (index, point) in trail.0.iter().enumerate() {
            let alpha = ((index + 1) as f32 / len as f32) * 0.55;
            let color = color.0.with_alpha(alpha);
            gizmos.circle_2d(point.0, 4.0, color);
            if let Some(next) = trail.0.get(index + 1) {
                gizmos.line_2d(point.0, next.0, color);
            }
        }
    }
}
