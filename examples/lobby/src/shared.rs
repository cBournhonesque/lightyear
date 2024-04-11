//! This module contains the shared code between the client and the server.
//!
//! The rendering code is here because you might want to run the example in host-server mode, where the server also acts as a client.
//! The simulation logic (movement, etc.) should be shared between client and server to guarantee that there won't be
//! mispredictions/rollbacks.
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::utils::Duration;
use bevy_mod_picking::debug::DebugPickingMode;
use bevy_mod_picking::DefaultPickingPlugins;

use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear::shared::config::Mode;

use crate::protocol::*;

pub fn shared_config(mode: Mode) -> SharedConfig {
    SharedConfig {
        client_send_interval: Duration::default(),
        server_send_interval: Duration::from_millis(40),
        // server_send_interval: Duration::from_millis(100),
        tick: TickConfig {
            tick_duration: Duration::from_secs_f64(1.0 / 64.0),
        },
        mode,
    }
}

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        if app.is_plugin_added::<RenderPlugin>() {
            app.add_plugins(DefaultPickingPlugins);
            app.add_systems(Startup, init);
            app.add_systems(Update, draw_boxes);
        }
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2dBundle::default());
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

/// System that draws the boxes of the player positions.
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
