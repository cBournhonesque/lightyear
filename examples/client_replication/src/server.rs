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
pub(crate) fn stepper.client_app()
    trigger: Trigger<OnAdd, ClientOf>,
    mut commands: Commands,
) {
    commands.entity(trigger.target()).insert((
        ReplicationReceiver::default(),
        ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ),
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
        if let Some(input) = &inputs.value {
            // NOTE: be careful to directly pass Mut<PlayerPosition>
            // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
            shared_movement_behaviour(position, input);
        }
    }
}

fn delete_player(
    mut commands: Commands,
    query: Query<(Entity, &ActionState<Inputs>), With<PlayerPosition>>,
) {
    for (entity, inputs) in query.iter() {
        if inputs.value.as_ref().is_some_and(|v| v == &Inputs::Delete) {
            // You can try 2 things here:
            // - either you consider that the client's action is correct, and you despawn the entity. This should get replicated
            //   to other clients.
            // - you decide that the client's despawn is incorrect, and you do not despawn the entity. Then the client's prediction
            //   should be rolled back, and the entity should not get despawned on client.
            commands.entity(entity).despawn();
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
    trigger: Trigger<OnAdd, PlayerPosition>,
    mut commands: Commands,
    player_query: Query<&Replicated>,
) {
    let entity = trigger.target();
    let Ok(replicated) = player_query.get(entity) else {
        return;
    };
    let client_entity = replicated.receiver;
    let client_id = replicated.from.unwrap();
    debug!("received player spawn event from {client_id:?}");

    if let Ok(mut e) = commands.get_entity(entity) {
        // if we receive a pre-predicted entity, only send the prepredicted component back
        // to the original client
        let mut overrides = ComponentReplicationOverrides::<PrePredicted>::default();
        overrides.global_override(ComponentReplicationOverride {
            disable: true,
            ..default()
        });
        overrides.override_for_sender(ComponentReplicationOverride {
            enable: true,
            ..default()
        }, client_entity);

        e.insert((
            // we want to replicate back to the original client, since they are using a pre-spawned entity
            Replicate::to_clients(NetworkTarget::All),
            // NOTE: even with a pre-spawned Predicted entity, we need to specify who will run prediction
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            OwnedBy {
                sender: client_entity,
                lifetime: Lifetime::SessionBased
            },
            // TODO: ControlledBy
            overrides,
        ));
    }
}

/// When we receive a replicated Cursor, replicate it to all other clients
pub(crate) fn replicate_cursors(
    trigger: Trigger<OnAdd, CursorPosition>,
    mut commands: Commands,
    cursor_query: Query<&Replicated>,
) {
    let entity = trigger.target();
    let Ok(replicated) = cursor_query.get(entity) else {
        return;
    };
    let client_id = replicated.from.unwrap();
    info!("received cursor spawn event from client: {client_id:?}");
    if let Ok(mut e) = commands.get_entity(entity) {
        // Cursor: replicate to others, interpolate for others
        e.insert((
            // do not replicate back to the client that owns the cursor!
            Replicate::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            // TODO: OwnedBy
        ));
    }
}
