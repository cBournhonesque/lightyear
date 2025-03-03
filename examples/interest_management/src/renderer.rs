use bevy::{color::palettes::basic::GREEN, prelude::*, render::RenderPlugin};
use lightyear::client::components::Confirmed;

use crate::protocol::*;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(Update, (draw_boxes, draw_circles));
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// System that draws the boxed of the player positions.
/// The components should be replicated from the server to the client
/// This time we will only draw the predicted/interpolated entities
pub(crate) fn draw_boxes(
    mut gizmos: Gizmos,
    players: Query<(&Position, &PlayerColor), Without<Confirmed>>,
) {
    for (position, color) in &players {
        gizmos.rect(
            Isometry3d::from_translation(Vec3::new(position.x, position.y, 0.0)),
            Vec2::ONE * 50.0,
            color.0,
        );
    }
}

/// System that draws circles
pub(crate) fn draw_circles(mut gizmos: Gizmos, circles: Query<&Position, With<CircleMarker>>) {
    for position in &circles {
        gizmos.circle_2d(Isometry2d::from_translation(position.0), 1.0, GREEN);
    }
}
