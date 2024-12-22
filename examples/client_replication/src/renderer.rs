use crate::protocol::*;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use lightyear::client::components::Confirmed;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(PostUpdate, draw_elements);
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// System that draws the player's boxes and cursors
pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<(&PlayerPosition, &PlayerColor), Without<Confirmed>>,
    cursors: Query<(&CursorPosition, &PlayerColor), Without<Confirmed>>,
) {
    for (position, color) in &players {
        gizmos.rect_2d(
            Isometry2d::from_translation(Vec2::new(position.x, position.y)),
            Vec2::ONE * 40.0,
            color.0,
        );
    }
    for (position, color) in &cursors {
        gizmos.circle_2d(
            Isometry2d::from_translation(Vec2::new(position.x, position.y)),
            15.0,
            color.0,
        );
    }
}
