use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use core::ops::Deref;
use leafwing_input_manager::action_state::ActionState;
use lightyear::connection::client::Connected;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

use crate::protocol::*;
use crate::shared;

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
    pub client_id_to_entity_id: HashMap<PeerId, Entity>,
}

// System to spawn the initial grid of dots
pub(crate) fn setup(mut commands: Commands) {
    // spawn dots in a grid
    for x in -NUM_CIRCLES..NUM_CIRCLES {
        for y in -NUM_CIRCLES..NUM_CIRCLES {
            let position = Position(Vec2::new(x as f32 * GRID_SIZE, y as f32 * GRID_SIZE));
            commands.spawn((
                position,
                Shape::Circle,
                ShapeChangeTimer(Timer::from_seconds(2.0, TimerMode::Repeating)),
                Replicate::to_clients(NetworkTarget::All),
                // A group with priority 2.0 will be replicated twice as often as a group with priority 1.0
                // in case the bandwidth is saturated.
                ReplicationGroup::default().set_priority(1.0 + y.abs() as f32),
            ));
        }
    }
}

/// Add the ReplicationSender component to new clients
pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    info!("New client connected: {:?}", trigger.entity);
    commands.entity(trigger.entity).insert((
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
        // limit to 3KB/s
        Transport::new(PriorityConfig::new(3000)),
    ));
}

/// Spawn the player entity when a client connects
pub(crate) fn handle_connected(
    trigger: On<Add, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(client_id) = query.get(trigger.entity) else {
        return;
    };
    let client_id = client_id.0;
    let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
    let s = 0.8;
    let l = 0.5;
    let color = Color::hsl(h, s, l);
    let entity = commands
        .spawn((
            PlayerId(client_id),
            Position(Vec2::splat(300.0)),
            PlayerColor(color),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            Name::from("Player".to_string()),
        ))
        .id();
    info!("Create entity {:?} for client {:?}", entity, client_id);
}

/// Read client inputs and move players
fn movement(
    mut query: Query<
        (&mut Position, &ActionState<Inputs>),
        // We don't want to apply inputs to the locally predicted entities
        Without<Predicted>,
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
