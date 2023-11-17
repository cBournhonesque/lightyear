use crate::protocol::*;
use bevy::prelude::*;
use lightyear_shared::plugin::config::LogConfig;
use lightyear_shared::{SharedConfig, TickConfig};
use std::time::Duration;
use tracing::Level;

pub fn shared_config() -> SharedConfig {
    SharedConfig {
        enable_replication: false,
        tick: TickConfig {
            tick_duration: Duration::from_millis(16),
        },
        log: LogConfig {
            level: Level::INFO,
            filter: "wgpu=error,wgpu_hal=error,naga=warn,bevy_app=info,bevy_render=warn"
                .to_string(),
        },
    }
}

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, draw_boxes);
    }
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(position: &mut PlayerPosition, input: &Inputs) {
    const MOVE_SPEED: f32 = 10.0;
    match input {
        Inputs::Direction(direction) => {
            if direction.up {
                position.y += MOVE_SPEED;
            }
            if direction.down {
                position.y -= MOVE_SPEED;
            }
            if direction.left {
                position.x -= MOVE_SPEED;
            }
            if direction.right {
                position.x += MOVE_SPEED;
            }
        }
        _ => {}
    }
}

/// System that draws the boxed of the player positions.
/// The components should be replicated from the server to the client
pub(crate) fn draw_boxes(mut gizmos: Gizmos, players: Query<(&PlayerPosition, &PlayerColor)>) {
    for (position, color) in &players {
        gizmos.rect(
            Vec3::new(position.x, position.y, 0.0),
            Quat::IDENTITY,
            Vec2::ONE * 50.0,
            color.0,
        );
    }
}

/// Input system: for now, lets server move the Player entity, and the components
/// should get replicated
///
pub(crate) fn input_system(
    mut player: Query<(Entity, &mut PlayerPosition)>,
    input: Res<Input<KeyCode>>,
    mut commands: Commands,
) {
    if let Ok((entity, mut position)) = player.get_single_mut() {
        const MOVE_SPEED: f32 = 10.0;
        if input.pressed(KeyCode::Right) {
            position.x += MOVE_SPEED;
        }
        if input.pressed(KeyCode::Left) {
            position.x -= MOVE_SPEED;
        }
        if input.pressed(KeyCode::Up) {
            position.y += MOVE_SPEED;
        }
        if input.pressed(KeyCode::Down) {
            position.y -= MOVE_SPEED;
        }
        if input.pressed(KeyCode::D) {
            commands.entity(entity).despawn();
        }
    }
}
