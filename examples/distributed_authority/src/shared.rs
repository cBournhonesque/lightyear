//! This module contains the shared code between the client and the server.
//!
//! The rendering code is here because you might want to run the example in host-server mode, where the server also acts as a client.
//! The simulation logic (movement, etc.) should be shared between client and server to guarantee that there won't be
//! mispredictions/rollbacks.

use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::utils::Duration;
use bevy_mod_picking::DefaultPickingPlugins;
use std::ops::{Deref, DerefMut};

use lightyear::prelude::client::{Confirmed, Interpolated};
use lightyear::prelude::*;
use lightyear::shared::config::Mode;

use crate::protocol::*;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<PlayerId>();
        app.register_type::<PlayerColor>();
        app.register_type::<Position>();
        app.register_type::<Speed>();

        // the protocol needs to be shared between the client and server
        app.add_plugins(ProtocolPlugin);
        if app.is_plugin_added::<RenderPlugin>() {
            app.add_plugins(DefaultPickingPlugins);
            app.add_systems(Startup, init);
            app.add_systems(Update, draw_boxes);
            app.add_systems(Update, draw_ball);
        }

        app.add_systems(FixedUpdate, ball_movement);
    }
}

fn init(mut commands: Commands) {
    commands.spawn(Camera2dBundle::default());
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(mut position: Mut<Position>, input: &Inputs) {
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

/// We move the ball only when we have authority over it.
/// The peer that has authority could be the Server, a Client or no one
pub(crate) fn ball_movement(
    mut balls: Query<
        (&mut Position, &mut Speed),
        (With<BallMarker>, With<HasAuthority>, Without<Interpolated>),
    >,
) {
    for (mut position, mut speed) in balls.iter_mut() {
        if position.y > 300.0 {
            speed.y = -1.0;
        }
        if position.y < -300.0 {
            speed.y = 1.0;
        }
        position.0 += speed.0;
    }
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
        gizmos.rect(
            Vec3::new(position.x, position.y, 0.0),
            Quat::IDENTITY,
            Vec2::ONE * 50.0,
            color.0,
        );
    }
}
