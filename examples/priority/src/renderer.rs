use bevy::{
    color::palettes::basic::{BLUE, GREEN, RED},
    prelude::*,
    render::RenderPlugin,
};
use lightyear::client::components::Confirmed;

use crate::protocol::*;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(Update, (draw_players, draw_props));
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// System that draws the player
/// The components should be replicated from the server to the client
/// This time we will only draw the predicted/interpolated entities
pub(crate) fn draw_players(
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

/// System that draws the props
pub(crate) fn draw_props(mut gizmos: Gizmos, props: Query<(&Position, &Shape)>) {
    for (position, shape) in props.iter() {
        match shape {
            Shape::Circle => {
                gizmos.circle_2d(
                    Isometry2d::from_translation(position.0),
                    crate::shared::PROP_SIZE,
                    GREEN,
                );
            }
            Shape::Triangle => {
                gizmos.linestrip_2d(
                    vec![
                        position.0 + Vec2::new(0.0, crate::shared::PROP_SIZE),
                        position.0 + Vec2::new(crate::shared::PROP_SIZE, -crate::shared::PROP_SIZE),
                        position.0
                            + Vec2::new(-crate::shared::PROP_SIZE, -crate::shared::PROP_SIZE),
                        position.0 + Vec2::new(0.0, crate::shared::PROP_SIZE),
                    ],
                    RED,
                );
            }
            Shape::Square => {
                gizmos.rect_2d(
                    Isometry2d::from_translation(position.0),
                    Vec2::splat(crate::shared::PROP_SIZE * 2.0),
                    BLUE,
                );
            }
        }
    }
}
