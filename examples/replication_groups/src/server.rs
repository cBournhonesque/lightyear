use crate::protocol::*;
use crate::shared::{shared_movement_behaviour, shared_tail_behaviour};
use bevy::prelude::*;
use lightyear::input::native::prelude::ActionState;
use lightyear::prediction::Predicted;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;
use std::collections::VecDeque;

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        // the simulation systems that can be rolled back must run in FixedUpdate
        app.add_systems(FixedUpdate, (movement, shared_tail_behaviour).chain());
        app.add_observer(handle_new_client);
        app.add_observer(handle_connections);
    }
}

pub(crate) fn handle_new_client(trigger: trigger<OnAdd, LinkOf>, mut commands: Commands) {
    commands
        .entity(trigger.target())
        .insert(ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ));
}

/// Server connection system, create a player upon connection
pub(crate) fn handle_connections(
    mut trigger: Trigger<OnAdd, Connected>,
    query: Query<&Connected, With<ClientOf>>,
    mut commands: Commands,
) {
    if let Ok(connected) = query.get(trigger.target()) {
        let client_id = connected.peer_id;
        // Generate pseudo random color from client id.
        let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
        let s = 0.8;
        let l = 0.5;
        let color = Color::hsl(h, s, l);
        let player_position = Vec2::ZERO;
        let player_entity = commands
            .spawn((
                PlayerId(client_id),
                PlayerPosition(player_position),
                PlayerColor(color),
                Replicate::to_clients(NetworkTarget::All),
                PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
                OwnedBy {
                    sender: trigger.target(),
                    lifetime: Lifetime::default(),
                },
            ))
            .id();

        let tail_length = 300.0;
        let default_direction = Direction::Up;
        let tail = default_direction.get_tail(player_position, tail_length);
        let mut points = VecDeque::new();
        points.push_front((tail, default_direction));
        let tail_entity = commands
            .spawn((
                PlayerParent(player_entity),
                TailPoints(points),
                TailLength(tail_length),
                ReplicateLike {
                    root: player_entity,
                },
            ))
            .id();
    }
}

/// Read client inputs and move players
pub(crate) fn movement(
    mut position_query: Query<
        (&mut PlayerPosition, &ActionState<Inputs>),
        // if we run in host-server mode, we don't want to apply this system to the local client's entities
        // because they are already moved by the client plugin
        (Without<Confirmed>, Without<Predicted>),
    >,
) {
    for (position, inputs) in position_query.iter_mut() {
        if let Some(input) = &inputs.value {
            // NOTE: be careful to directly pass Mut<PlayerPosition>
            // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
            shared_movement_behaviour(position, input);
        }
    }
}
