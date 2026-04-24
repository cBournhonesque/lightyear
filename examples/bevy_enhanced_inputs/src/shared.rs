//! This module contains the shared code between the client and the server.
//!
//! The simulation logic (movement, etc.) should be shared between client and server to guarantee that there won't be
//! mispredictions/rollbacks.
use crate::protocol::*;
use bevy::prelude::*;
use lightyear::input::bei::prelude::{Action, ActionOf, Bindings, Cardinal};
use lightyear::prelude::*;
use lightyear_examples_common::shared::SharedSettings;

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ProtocolPlugin);
        app.add_systems(FixedPostUpdate, emit_fixed_post_positions);
        app.add_systems(Update, emit_confirmed_positions);
        app.add_systems(PostUpdate, emit_interpolated_positions);
    }
}

/// Deterministic hash for PreSpawned action entities.
/// Uses the client's PeerId and a salt to produce the same hash on both client and server,
/// regardless of spawn tick.
pub(crate) fn action_prespawn_hash(client_id: PeerId, salt: u64) -> u64 {
    client_id
        .to_bits()
        .wrapping_mul(6364136223846793005)
        .wrapping_add(salt)
}

/// Spawn action entities for a player. Called on both client and server.
pub(crate) fn spawn_action_entities(
    commands: &mut Commands,
    player_entity: Entity,
    client_id: PeerId,
    is_server: bool,
) {
    let hash = action_prespawn_hash(client_id, 1);
    let mut action = commands.spawn((
        ActionOf::<Player>::new(player_entity),
        Action::<Movement>::new(),
        Bindings::spawn(Cardinal::wasd_keys()),
        PreSpawned::new(hash),
    ));
    if is_server {
        #[cfg(feature = "server")]
        action.insert(Replicate::to_clients(NetworkTarget::Single(client_id)));
    } else {
        action.insert(lightyear::prelude::input::bei::InputMarker::<Player>::default());
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

pub(crate) fn emit_confirmed_positions(
    timeline: Res<LocalTimeline>,
    players: Query<(Entity, &PlayerPosition), (With<PlayerId>, Changed<PlayerPosition>)>,
) {
    let tick = timeline.tick();
    for (entity, position) in players.iter() {
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::Update,
            "Update",
            "confirmed_position",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            "confirmed position updated"
        );
    }
}

pub(crate) fn emit_interpolated_positions(
    timeline: Res<LocalTimeline>,
    players: Query<(Entity, &PlayerPosition), With<Interpolated>>,
) {
    let tick = timeline.tick();
    for (entity, position) in players.iter() {
        lightyear_debug_event!(
            DebugCategory::Interpolation,
            DebugSamplePoint::PostUpdate,
            "PostUpdate",
            "interpolated_position",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            "interpolated position"
        );
    }
}

pub(crate) fn emit_fixed_post_positions(
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
        lightyear_debug_event!(
            DebugCategory::Component,
            DebugSamplePoint::FixedPostUpdate,
            "FixedPostUpdate",
            "fixed_post_position",
            tick = ?tick,
            entity = ?entity,
            position = ?position,
            // ?action_state,
            // %input_buffer,
            "Player after movement"
        );
    }
}
