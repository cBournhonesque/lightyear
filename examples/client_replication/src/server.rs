use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_movement_behaviour};
use bevy::prelude::*;
use core::time::Duration;
use lightyear::input::native::prelude::ActionState;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(replicate_cursors);
        app.add_observer(replicate_players);
        app.add_observer(handle_new_client);
        app.add_systems(FixedUpdate, (movement, delete_player));
    }
}

/// When a new client tries to connect to a server, an entity is created for it with the `ClientOf` component.
/// This entity represents the connection between the server and that client.
///
/// You can add additional components to update the connection. In this case we will add a `ReplicationSender` that
/// will enable us to replicate local entities to that client.
pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert((
        ReplicationReceiver::default(),
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
        Name::from("ClientOf"),
    ));
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
        // NOTE: be careful to directly pass Mut<PlayerPosition>
        // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
        shared_movement_behaviour(position, inputs);
    }
}

fn delete_player(
    mut commands: Commands,
    query: Query<(Entity, &ActionState<Inputs>), With<PlayerPosition>>,
) {
    for (entity, inputs) in query.iter() {
        if inputs.0 == Inputs::Delete {
            // You can try 2 things here:
            // - either you consider that the client's action is correct, and you despawn the entity. This should get replicated
            //   to other clients.
            // - you decide that the client's despawn is incorrect, and you do not despawn the entity. Then the client's prediction
            //   should be rolled back, and the entity should not get despawned on client.
            commands.entity(entity).despawn();
            info!("Despawn the confirmed player {entity:?} on the server");
        }
    }
}

// Replicate the pre-predicted entities back to the client.
//
// The objective was to create a normal 'predicted' entity directly in the client timeline, instead
// of having to create the entity in the server timeline and wait for it to be replicated.
// Note that this needs to run before FixedUpdate, since we handle client inputs in the FixedUpdate schedule (subject to change)
// And we want to handle deletion properly
pub(crate) fn replicate_players(
    // We add an observer on both Cursor and Replicated because
    // in host-server mode, Replicated is not present on the entity when
    // CursorPosition is added. (Replicated gets added slightly after by an observer)
    trigger: On<Add, (PlayerPosition, Replicated)>,
    mut commands: Commands,
    player_query: Query<&Replicated, With<PlayerPosition>>,
) {
    let entity = trigger.entity;
    let Ok(replicated) = player_query.get(entity) else {
        return;
    };
    let client_entity = replicated.receiver;
    let client_id = replicated.from;

    if let Ok(mut e) = commands.get_entity(entity) {
        info!("received player spawn event from {client_id:?}");
        e.insert((
            // we want to replicate back to the original client, since they are using a pre-spawned entity
            Replicate::to_clients(NetworkTarget::All),
            // NOTE: even with a pre-spawned Predicted entity, we need to specify who will run prediction
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            ControlledBy {
                owner: client_entity,
                lifetime: Lifetime::SessionBased,
            },
            // if we receive a pre-predicted entity, only send the prepredicted component back
            // to the original client
            ComponentReplicationOverrides::<PrePredicted>::default()
                .disable_all()
                .enable_for(client_entity),
        ));
    }
}

/// When we receive a replicated Cursor, replicate it to all other clients
pub(crate) fn replicate_cursors(
    // We add an observer on both Cursor and Replicated because
    // in host-server mode, Replicated is not present on the entity when
    // CursorPosition is added. (Replicated gets added slightly after by an observer)
    trigger: On<Add, (CursorPosition, Replicated)>,
    mut commands: Commands,
    cursor_query: Query<&Replicated, With<CursorPosition>>,
) {
    let entity = trigger.entity;
    let Ok(replicated) = cursor_query.get(entity) else {
        return;
    };
    let client_id = replicated.from;
    info!("received cursor spawn event from client: {client_id:?}");
    if let Ok(mut e) = commands.get_entity(entity) {
        // Cursor: replicate to others, interpolate for others
        e.insert((
            // do not replicate back to the client that owns the cursor!
            Replicate::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            ControlledBy {
                owner: replicated.receiver,
                lifetime: Lifetime::SessionBased,
            },
        ));
    }
}
