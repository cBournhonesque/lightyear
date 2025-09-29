use crate::protocol::*;
use bevy::prelude::*;

#[derive(Clone)]
pub struct ExampleRendererPlugin;

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(Update, draw_snakes);
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// System that draws the boxes of the player positions.
/// The components should be replicated from the server to the client
pub(crate) fn draw_snakes(
    mut gizmos: Gizmos,
    players: Query<(Entity, &PlayerPosition, &PlayerColor)>,
    tails: Query<(Entity, &PlayerParent, &TailPoints)>,
) {
    for (tail, parent, points) in tails.iter() {
        debug!("drawing snake with parent: {:?}", parent.0);
        let Ok((head, position, color)) = players.get(parent.0) else {
            error!(?tail, parent = ?parent.0, "Tail entity has no parent entity!");
            continue;
        };
        // draw the head
        gizmos.rect(
            Isometry3d::from_translation(Vec3::new(position.x, position.y, 0.0)),
            Vec2::ONE * 20.0,
            color.0,
        );
        // draw the first line
        gizmos.line_2d(position.0, points.0.front().unwrap().0, color.0);
        if position.0.x != points.0.front().unwrap().0.x
            && position.0.y != points.0.front().unwrap().0.y
        {
            debug!("DIAGONAL");
        }
        // draw the rest of the lines
        for (start, end) in points.0.iter().zip(points.0.iter().skip(1)) {
            gizmos.line_2d(start.0, end.0, color.0);
            if start.0.x != end.0.x && start.0.y != end.0.y {
                debug!("DIAGONAL");
            }
        }
    }
}
