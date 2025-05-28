use crate::protocol::*;
use bevy::prelude::*;
use lightyear::prelude::Confirmed;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(Update, draw_boxes);
        app.add_systems(Update, draw_ball);
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// We draw:
/// - on the server: we always draw the ball
/// - on the client: we draw the interpolated ball (when the client has authority,
///   the confirmed updates are added to the component history, instead of the server updates)
// TODO: it can be a bit tedious to have the check if we want to draw the interpolated or the confirmed ball.
//  if we have authority, should the interpolated ball become the same as Confirmed?
pub(crate) fn draw_ball(
    mut gizmos: Gizmos,
    balls: Query<(&Position, &PlayerColor), (With<BallMarker>, Without<Confirmed>)>,
) {
    for (position, color) in balls.iter() {
        gizmos.circle_2d(position.0, 25.0, color.0);
    }
}

/// System that draws the boxes of the player positions.
/// The components should be replicated from the server to the client
pub(crate) fn draw_boxes(
    mut gizmos: Gizmos,
    players: Query<(&Position, &PlayerColor), (Without<BallMarker>, Without<Confirmed>)>,
) {
    for (position, color) in &players {
        gizmos.rect_2d(
            Isometry2d::from_translation(position.0),
            Vec2::ONE * 50.0,
            color.0,
        );
    }
}
