use bevy::color::palettes::css::GREEN;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::utils::Duration;
use leafwing_input_manager::action_state::ActionState;
use std::ops::Deref;

use lightyear::client::components::Confirmed;
use lightyear::prelude::*;

use crate::protocol::*;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        if app.is_plugin_added::<RenderPlugin>() {
            app.add_systems(Startup, init);
            app.add_systems(Update, (draw_boxes, draw_circles));
        }
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2dBundle::default());
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(mut position: Mut<Position>, input: &ActionState<Inputs>) {
    const MOVE_SPEED: f32 = 10.0;
    if input.pressed(&Inputs::Up) {
        position.y += MOVE_SPEED;
    }
    if input.pressed(&Inputs::Down) {
        position.y -= MOVE_SPEED;
    }
    if input.pressed(&Inputs::Left) {
        position.x -= MOVE_SPEED;
    }
    if input.pressed(&Inputs::Right) {
        position.x += MOVE_SPEED;
    }
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
            Vec3::new(position.x, position.y, 0.0),
            Quat::IDENTITY,
            Vec2::ONE * 50.0,
            color.0,
        );
    }
}

/// System that draws circles
pub(crate) fn draw_circles(mut gizmos: Gizmos, circles: Query<&Position, With<CircleMarker>>) {
    for position in &circles {
        gizmos.circle_2d(*position.deref(), 1.0, GREEN);
    }
}

/// Generate a color from the `ClientId`
pub(crate) fn color_from_id(client_id: ClientId) -> Color {
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 1.0;
    let l = 0.5;
    Color::hsl(h, s, l)
}
