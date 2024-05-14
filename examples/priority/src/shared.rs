use std::ops::Deref;

use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::utils::Duration;
use bevy_screen_diagnostics::{Aggregate, ScreenDiagnostics, ScreenDiagnosticsPlugin};
use leafwing_input_manager::action_state::ActionState;

use lightyear::prelude::client::Confirmed;
use lightyear::prelude::*;
use lightyear::transport::io::IoDiagnosticsPlugin;

use crate::protocol::*;

const MOVE_SPEED: f32 = 10.0;
const PROP_SIZE: f32 = 5.0;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        if app.is_plugin_added::<RenderPlugin>() {
            app.add_systems(Startup, init);
            app.add_systems(Update, (draw_players, draw_props));
            // diagnostics
            app.add_systems(Startup, setup_diagnostic);
            app.add_plugins(ScreenDiagnosticsPlugin::default());
        }

        // movement
        app.add_systems(FixedUpdate, player_movement);
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2dBundle::default());
}

fn setup_diagnostic(mut onscreen: ResMut<ScreenDiagnostics>) {
    onscreen
        .add("KB/S in".to_string(), IoDiagnosticsPlugin::BYTES_IN)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:.2}"));
    onscreen
        .add("KB/s out".to_string(), IoDiagnosticsPlugin::BYTES_OUT)
        .aggregate(Aggregate::Average)
        .format(|v| format!("{v:.2}"));
}

/// Read client inputs and move players
pub(crate) fn player_movement(
    mut position_query: Query<(&mut Position, &ActionState<Inputs>), Without<Confirmed>>,
) {
    for (mut position, input) in position_query.iter_mut() {
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
            Vec3::new(position.x, position.y, 0.0),
            Quat::IDENTITY,
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
                gizmos.circle_2d(*position.deref(), PROP_SIZE, Color::GREEN);
            }
            Shape::Triangle => {
                gizmos.linestrip_2d(
                    vec![
                        *position.deref() + Vec2::new(0.0, PROP_SIZE),
                        *position.deref() + Vec2::new(PROP_SIZE, -PROP_SIZE),
                        *position.deref() + Vec2::new(-PROP_SIZE, -PROP_SIZE),
                        *position.deref() + Vec2::new(0.0, PROP_SIZE),
                    ],
                    Color::RED,
                );
            }
            Shape::Square => {
                gizmos.rect_2d(
                    *position.deref(),
                    0.0,
                    Vec2::splat(PROP_SIZE * 2.0),
                    Color::BLUE,
                );
            }
        }
    }
}
