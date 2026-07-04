use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Action, ActionOf, Fire};
use core::ops::Deref;
use lightyear::connection::client::Connected;
use lightyear::connection::host::{HostClient, HostServer};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

use crate::automation::AutomationServerPlugin;
use crate::protocol::*;
use crate::shared;

pub struct ExampleServerPlugin;

#[derive(Component)]
pub(crate) struct ServerAction;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationServerPlugin);
        app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));
        app.init_resource::<Global>();
        app.add_systems(Startup, setup);
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
        app.add_observer(movement);
        app.add_systems(Update, (tick_timers, update_props).chain());
        app.add_systems(
            PostUpdate,
            update_priority_maps.before(ReplicationSystems::Send),
        );
    }
}

const GRID_SIZE: f32 = 20.0;
const GRID_RADIUS: i32 = 3;
const HIGH_PROP_PRIORITY: f32 = 1.0;
const MEDIUM_PROP_PRIORITY: f32 = 0.025;
const LOW_PROP_PRIORITY: f32 = 0.0125;

#[derive(Resource, Default)]
pub(crate) struct Global {
    pub client_id_to_entity_id: HashMap<PeerId, Entity>,
}

// System to spawn the initial grid of dots
pub(crate) fn setup(mut commands: Commands) {
    // spawn dots in a grid
    for x in -GRID_RADIUS..=GRID_RADIUS {
        for y in -GRID_RADIUS..=GRID_RADIUS {
            let position = Position(Vec2::new(x as f32 * GRID_SIZE, y as f32 * GRID_SIZE));
            let mut entity = commands.spawn((
                position,
                Shape::Circle,
                ShapeChangeTimer(Timer::from_seconds(2.0, TimerMode::Repeating)),
                Replicate::to_clients(NetworkTarget::All),
            ));
            match y.abs() {
                0 => {
                    entity.insert(LowPriority);
                }
                1 => {
                    entity.insert(MediumPriority);
                }
                _ => {
                    entity.insert(HighPriority);
                }
            }
        }
    }
}

/// Add the ReplicationSender component to new clients
pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    info!("New client connected: {:?}", trigger.entity);
    commands.entity(trigger.entity).insert((
        ReplicationSender,
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
            Player,
            PlayerId(client_id),
            Position(Vec2::splat(300.0)),
            PlayerColor(color),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            ControlledBy {
                owner: trigger.entity,
                lifetime: Default::default(),
            },
            Name::from("Player".to_string()),
        ))
        .id();
    info!("Create entity {:?} for client {:?}", entity, client_id);
    spawn_action_entities(&mut commands, entity);
}

/// Spawn the BEI action entity for a player.
///
/// The server owns and replicates action entities so the owning client can
/// target them in input messages.
fn spawn_action_entities(commands: &mut Commands, player_entity: Entity) {
    commands.spawn((
        ActionOf::<Player>::new(player_entity),
        Action::<Movement>::new(),
        ReplicateLike {
            root: player_entity,
        },
        ServerAction,
    ));
}

/// Read client inputs and move players
fn movement(
    trigger: On<Fire<Movement>>,
    host_server: Query<(), With<HostServer>>,
    server_actions: Query<(), (With<Action<Movement>>, With<ServerAction>)>,
    controlled_by: Query<&ControlledBy>,
    host_clients: Query<(), With<HostClient>>,
    mut position_query: Query<&mut Position>,
) {
    let is_host_server = !host_server.is_empty();
    if is_host_server && !server_actions.contains(trigger.action) {
        return;
    }
    if is_host_server {
        if let Ok(controlled_by) = controlled_by.get(trigger.context) {
            if host_clients.get(controlled_by.owner).is_ok() {
                return;
            }
        }
    }
    if let Ok(position) = position_query.get_mut(trigger.context) {
        shared::shared_movement_behaviour(position, trigger.value);
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

fn update_priority_maps(
    props: Query<
        (
            Entity,
            Has<HighPriority>,
            Has<MediumPriority>,
            Has<LowPriority>,
        ),
        With<Shape>,
    >,
    mut clients: Query<&mut PriorityMap, With<ClientOf>>,
) {
    for mut priority_map in &mut clients {
        for (entity, high_priority, medium_priority, low_priority) in &props {
            let priority = if high_priority {
                HIGH_PROP_PRIORITY
            } else if medium_priority {
                MEDIUM_PROP_PRIORITY
            } else if low_priority {
                LOW_PROP_PRIORITY
            } else {
                continue;
            };
            priority_map.insert(entity, priority);
        }
    }
}
