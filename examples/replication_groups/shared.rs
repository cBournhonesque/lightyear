use crate::protocol::Direction;
use crate::protocol::*;
use bevy::prelude::*;
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use lightyear::prelude::*;
use std::time::Duration;
use tracing::Level;

pub fn shared_config() -> SharedConfig {
    SharedConfig {
        enable_replication: true,
        client_send_interval: Duration::default(),
        server_send_interval: Duration::from_millis(40),
        // server_send_interval: Duration::from_millis(100),
        tick: TickConfig {
            tick_duration: Duration::from_secs_f64(1.0 / 64.0),
        },
        log: LogConfig {
            level: Level::INFO,
            filter: "wgpu=error,wgpu_hal=error,naga=warn,bevy_app=info,bevy_render=warn,quinn=warn"
                .to_string(),
        },
    }
}

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // app.add_plugins(WorldInspectorPlugin::new());
        app.add_systems(Update, draw_snakes);
    }
}

// head
// snake

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(position: &mut PlayerPosition, input: &Inputs) {
    const MOVE_SPEED: f32 = 10.0;
    match input {
        Inputs::Direction(direction) => match direction {
            Direction::Up => position.y += MOVE_SPEED,
            Direction::Down => position.y -= MOVE_SPEED,
            Direction::Left => position.x -= MOVE_SPEED,
            Direction::Right => position.x += MOVE_SPEED,
        },
        _ => {}
    }
}

// This system defines how we update the player's tails when we the head is updated
pub(crate) fn shared_tail_behaviour(
    player_position: Query<&PlayerPosition>,
    mut tails: Query<(&mut TailPoints, &PlayerParent, &TailLength)>,
) {
    for (mut points, parent, length) in tails.iter_mut() {
        let Ok(parent_position) = player_position.get(parent.0) else {
            panic!("Tail entity has no parent entity!");
        };
        // Update the front if the head turned
        let (front_pos, front_dir) = points.0.front().unwrap().clone();
        // NOTE: we do not deal with diagonal directions in this example
        let front_direction = Direction::from_points(front_pos, parent_position.0);
        // if the head is going in a new direction, add a new point to the front
        if front_direction.map_or(true, |dir| dir != front_dir) {
            trace!(
                old_front_dir = ?front_dir,
                new_front_dir = ?front_direction,
                "creating new inflection point");
            let inflection_pos = match front_dir {
                Direction::Up | Direction::Down => Vec2::new(front_pos.x, parent_position.y),
                Direction::Left | Direction::Right => Vec2::new(parent_position.x, front_pos.y),
            };
            let new_front_dir = Direction::from_points(inflection_pos, parent_position.0).unwrap();
            points.0.push_front((inflection_pos, new_front_dir));
            trace!(?points, "new points");
        }

        // Update the back
        // remove the back points that are above the length
        points.shorten_back(parent_position.0, length.0);
    }
}

/// System that draws the boxed of the player positions.
/// The components should be replicated from the server to the client
pub(crate) fn draw_snakes(
    mut gizmos: Gizmos,
    players: Query<(&PlayerPosition, &PlayerColor)>,
    tails: Query<(&PlayerParent, &TailPoints)>,
) {
    for (parent, points) in tails.iter() {
        let Ok((position, color)) = players.get(parent.0) else {
            panic!("Tail entity has no parent entity!");
        };
        // draw the head
        gizmos.rect(
            Vec3::new(position.x, position.y, 0.0),
            Quat::IDENTITY,
            Vec2::ONE * 20.0,
            color.0,
        );
        // draw the first line
        gizmos.line_2d(position.0, points.0.front().unwrap().0, color.0);
        // draw the rest of the lines
        for (start, end) in points.0.iter().zip(points.0.iter().skip(1)) {
            gizmos.line_2d(start.0, end.0, color.0);
        }
    }
}
