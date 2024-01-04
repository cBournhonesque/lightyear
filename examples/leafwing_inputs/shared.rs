use crate::protocol::*;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use leafwing_input_manager::prelude::ActionState;
use lightyear::prelude::client::Confirmed;
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
        if app.is_plugin_added::<RenderPlugin>() {
            app.add_systems(Update, draw_elements);
        }
    }
}

// Generate pseudo-random color from id
pub(crate) fn color_from_id(client_id: ClientId) -> Color {
    let h = ((client_id * 90) % 360) as f32;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(
    mut position: Mut<Position>,
    action: &ActionState<PlayerActions>,
) {
    const MOVE_SPEED: f32 = 10.0;
    if action.pressed(PlayerActions::Up) {
        position.y += MOVE_SPEED;
    }
    if action.pressed(PlayerActions::Down) {
        position.y -= MOVE_SPEED;
    }
    if action.pressed(PlayerActions::Left) {
        position.x -= MOVE_SPEED;
    }
    if action.pressed(PlayerActions::Right) {
        position.x += MOVE_SPEED;
    }
}

/// System that draws the player's boxes and cursors
pub(crate) fn draw_elements(
    mut gizmos: Gizmos,
    players: Query<(&Position, &ColorComponent), (Without<Confirmed>, Without<BallMarker>)>,
    cursors: Query<(&Position, &ColorComponent), (Without<Confirmed>, With<BallMarker>)>,
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
