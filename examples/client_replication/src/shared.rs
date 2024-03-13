use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::utils::Duration;

use lightyear::prelude::client::Confirmed;
use lightyear::prelude::*;

use crate::protocol::*;

pub fn shared_config() -> SharedConfig {
    SharedConfig {
        client_send_interval: Duration::default(),
        server_send_interval: Duration::from_millis(40),
        // server_send_interval: Duration::from_millis(100),
        tick: TickConfig {
            tick_duration: Duration::from_secs_f64(1.0 / 64.0),
        },
    }
}

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        if app.is_plugin_added::<RenderPlugin>() {
            app.add_systems(PostUpdate, draw_elements);
        }
    }
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: ClientId) -> Color {
    let h = (((client_id.wrapping_mul(90)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(mut position: Mut<PlayerPosition>, input: &Inputs) {
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

/// System that draws the player's boxes and cursors
pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<(&PlayerPosition, &PlayerColor), Without<Confirmed>>,
    cursors: Query<(&CursorPosition, &PlayerColor), Without<Confirmed>>,
) {
    for (position, color) in &players {
        gizmos.rect_2d(
            Vec2::new(position.x, position.y),
            0.0,
            Vec2::ONE * 40.0,
            color.0,
        );
    }
    for (position, color) in &cursors {
        gizmos.circle_2d(Vec2::new(position.x, position.y), 15.0, color.0);
    }
}
