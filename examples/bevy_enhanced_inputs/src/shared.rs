//! This module contains the shared code between the client and the server.
//!
//! The simulation logic (movement, etc.) should be shared between client and server to guarantee that there won't be
//! mispredictions/rollbacks.
use crate::protocol::*;
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SharedSettings;

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.add_systems(FixedPostUpdate, fixed_post_log);
        app.add_systems(Update, confirmed_log);
        app.add_systems(PostUpdate, interpolate_log);
    }
}

pub const SHARED_SETTINGS: SharedSettings = SharedSettings {
    protocol_id: 0,
    private_key: [0; 32],
};

// This system defines how we update the player's positions when we receive an input
pub(crate) fn shared_movement_behaviour(mut position: Mut<PlayerPosition>, input: Vec2) {
    const MOVE_SPEED: f32 = 10.0;
    position.0.y += input.y * MOVE_SPEED;
    position.0.x += input.x * MOVE_SPEED;
}

pub(crate) fn confirmed_log(
    timeline: Res<LocalTimeline>,
    players: Query<(Entity, &Confirmed<PlayerPosition>), Changed<Confirmed<PlayerPosition>>>,
) {
    let tick = timeline.tick();
    for status in players.iter() {
        trace!(?tick, ?status, "Confirmed Updated");
    }
}

pub(crate) fn interpolate_log(
    timeline: Res<LocalTimeline>,
    players: Query<(Entity, &PlayerPosition), With<Interpolated>>,
) {
    let tick = timeline.tick();
    for (entity, position) in players.iter() {
        trace!(?tick, ?entity, ?position, "Interpolation");
    }
}

pub(crate) fn fixed_post_log(
    timeline: Res<LocalTimeline>,
    players: Query<
        (Entity, &PlayerPosition),
        // (Entity, &PlayerPosition, &ActionState<Inputs>, &InputBuffer<ActionState<Inputs>>),
        With<PlayerId>,
    >,
) {
    let tick = timeline.tick();
    // for (entity, position, action_state, input_buffer) in players.iter() {
    for (entity, position) in players.iter() {
        trace!(
            ?tick,
            ?entity,
            ?position,
            // ?action_state,
            // %input_buffer,
            "Player after movement"
        );
    }
}
