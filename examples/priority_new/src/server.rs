use bevy::prelude::*;
use bevy::utils::HashMap;
use core::ops::Deref;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::connection::client::Connected; // Import Connected
use lightyear_examples_common_new::shared::SEND_INTERVAL; // Import SEND_INTERVAL

use crate::protocol::*;
use crate::shared; // Assuming shared movement logic exists

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Global>();
        app.add_systems(Startup, setup);
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, movement);
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
        app.add_systems(Update, (tick_timers, update_props).chain());
    }
}

const GRID_SIZE: f32 = 20.0;
const NUM_CIRCLES: i32 = 6;

#[derive(Resource, Default)]
pub(crate) struct Global {
    // Updated to PeerId
    pub client_id_to_entity_id: HashMap<PeerId, Entity>,
}

// System to spawn the initial grid of dots
pub(crate) fn setup(mut commands: Commands) {
    // spawn dots in a grid
    for x in -NUM_CIRCLES..NUM_CIRCLES {
        for y in -NUM_CIRCLES..NUM_CIRCLES {
            let position = Position(Vec2::new(x as f32 * GRID_SIZE, y as f32 * GRID_SIZE));
            let mut replicate = Replicate::to_clients(NetworkTarget::All);
            // A ReplicationGroup is replicated together as a single message, so the priority should
            // be set on the group.
            // A group with priority 2.0 will be replicated twice as often as a group with priority 1.0
            // in case the bandwidth is saturated.
            replicate.group = ReplicationGroup::default().set_priority(1.0 + y.abs() as f32);

            commands.spawn((
                position,
                Shape::Circle,
                ShapeChangeTimer(Timer::from_seconds(2.0, TimerMode::Repeating)),
                replicate,
            ));
        }
    }
}

/// Add the ReplicationSender component to new clients
pub(crate) fn handle_new_client(
    trigger: Trigger<OnAdd, ClientOf>,
    mut commands: Commands,
) {
    commands.entity(trigger.target()).insert(
        ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ),
    );
}

/// Spawn the player entity when a client connects
pub(crate) fn handle_connected(
    trigger: Trigger<OnAdd, Connected>,
    mut query: Query<&Connected, With<ClientOf>>,
    mut commands: Commands,
) {
    let connected = query.get(trigger.target()).unwrap();
    let client_id = connected.peer_id; // Use PeerId
    let entity = commands
        .spawn((
            PlayerBundle::new(client_id, Vec2::splat(300.0)),
            // we replicate the Player entity to all clients that are connected to this server
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
        ))
        .id();
    info!("Create entity {:?} for client {:?}", entity, client_id);
}


/// Read client inputs and move players
fn movement(
    mut query: Query<
        (&mut Position, &ActionState<Inputs>),
        // We don't want to apply inputs to the locally predicted entities
        (Without<Confirmed>, Without<Predicted>),
    >,
) {
    for (position, action_state) in query.iter_mut() {
        // Use the shared movement function, adapted for ActionState
        shared::shared_movement_behaviour(position, action_state);
    }
}


pub(crate) fn tick_timers(mut timers: Query<&mut ShapeChangeTimer>, time: Res<Time>) {
    for mut timer in timers.iter_mut() {
        timer.tick(time.delta());
    }
}

pub(crate) fn update_props(mut props: Query<(&mut Shape, &ShapeChangeTimer)>) {
    for (mut shape, timer) in props.iter_mut() {
        if timer.just_finished() {
            if shape.deref() == &Shape::Circle {
                *shape = Shape::Triangle;
            } else if shape.deref() == &Shape::Triangle {
                *shape = Shape::Square;
            } else if shape.deref() == &Shape::Square {
                *shape = Shape::Circle;
            }
        }
    }
}
