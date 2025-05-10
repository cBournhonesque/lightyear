//! This module contains the shared code between the client and the server.
//!
//! The simulation logic (movement, etc.) should be shared between client and server to guarantee that there won't be
//! mispredictions/rollbacks.
use crate::protocol::*;
use bevy::prelude::*;
use lightyear::connection::client_of::ClientOf;
use lightyear::input::input_buffer::InputBuffer;
use lightyear::input::native::prelude::ActionState;
use lightyear::prelude::{Client, Confirmed, LocalTimeline, NetworkTimeline, Rollback};


pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.add_systems(FixedPostUpdate, fixed_post_log);
    }
}

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(mut position: Mut<PlayerPosition>, input: &Inputs) {
    const MOVE_SPEED: f32 = 10.0;
    let Inputs::Direction(direction) = input;
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

pub(crate) fn fixed_post_log(
    timeline: Single<(&LocalTimeline, Has<Rollback>), Or<(With<Client>, Without<ClientOf>)>>,
    players: Query<
        (Entity, &PlayerPosition, &ActionState<Inputs>, &InputBuffer<ActionState<Inputs>>),
        (Without<Confirmed>, With<PlayerId>),
    >,
) {
    let (timeline, rollback) = timeline.into_inner();
    let tick = timeline.tick();
    for (entity, position, action_state, input_buffer) in players.iter() {
        info!(
            ?rollback,
            ?tick,
            ?entity,
            ?position,
            ?action_state,
            %input_buffer,
            "Player after movement"
        );
    }
}
