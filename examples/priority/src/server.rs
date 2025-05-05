use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use core::ops::Deref;
pub use lightyear::prelude::server::*;
use lightyear::prelude::*;

use crate::protocol::*;

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Global>();
        app.add_systems(Startup, init);
        app.add_systems(
            Update,
            (handle_connections, (tick_timers, update_props).chain()),
        );
    }
}

const GRID_SIZE: f32 = 20.0;
const NUM_CIRCLES: i32 = 6;

#[derive(Resource, Default)]
pub(crate) struct Global {
    pub client_id_to_entity_id: HashMap<ClientId, Entity>,
}

pub(crate) fn init(mut commands: Commands) {
    commands.start_server();
    // spawn dots in a grid
    for x in -NUM_CIRCLES..NUM_CIRCLES {
        for y in -NUM_CIRCLES..NUM_CIRCLES {
            commands.spawn((
                Position(Vec2::new(x as f32 * GRID_SIZE, y as f32 * GRID_SIZE)),
                Shape::Circle,
                ShapeChangeTimer(Timer::from_seconds(2.0, TimerMode::Repeating)),
                Replicate {
                    // A ReplicationGroup is replicated together as a single message, so the priority should
                    // be set on the group.
                    // A group with priority 2.0 will be replicated twice as often as a group with priority 1.0
                    // in case the bandwidth is saturated.
                    // The priority can be sent when the entity is spawned; if multiple entities in the same group have
                    // different priorities, the latest set priority will be used.
                    // After the entity is spawned, you can update the priority using the ConnectionManager::update_priority method.
                    group: ReplicationGroup::default().set_priority(1.0 + y.abs() as f32),
                    ..default()
                },
            ));
        }
    }
}

/// Server connection system, create a player upon connection
pub(crate) fn handle_connections(
    mut connections: EventReader<ConnectEvent>,
    mut commands: Commands,
) {
    for connection in connections.read() {
        let client_id = connection.client_id;
        let entity = commands.spawn(PlayerBundle::new(client_id, Vec2::splat(300.0)));
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
