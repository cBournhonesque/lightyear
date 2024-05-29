use std::f32::consts::TAU;

use bevy::prelude::*;
use bevy::utils::Duration;
use bevy::utils::HashMap;
use bevy_xpbd_2d::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::inputs::leafwing::InputMessage;
use lightyear::prelude::client::{Confirmed, Predicted};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

use crate::protocol::*;
use crate::shared;
use crate::shared::spawn_bullet;
use crate::shared::ApplyInputsQuery;
use crate::shared::{color_from_id, shared_movement_behaviour, FixedSet};

// Plugin for server-specific logic
pub struct ExampleServerPlugin {
    pub(crate) predict_all: bool,
}

#[derive(Resource)]
pub struct Global {
    predict_all: bool,
}

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Global {
            predict_all: self.predict_all,
        });

        app.add_systems(Startup, (start_server, init));
        app.add_systems(
            PreUpdate,
            // this system will replicate the inputs of a client to other clients
            // so that a client can predict other clients
            replicate_inputs.after(MainSet::EmitEvents),
        );
        // spawn player entities once they connect
        app.add_systems(Update, handle_connections);
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(
            FixedUpdate,
            (player_movement, shared::shared_player_firing)
                .chain()
                .in_set(FixedSet::Main),
        );
    }
}

/// System to start the server at Startup
fn start_server(mut commands: Commands) {
    commands.start_server();
}

fn init(mut commands: Commands) {
    commands.spawn(
        TextBundle::from_section(
            "Server",
            TextStyle {
                font_size: 30.0,
                color: Color::WHITE,
                ..default()
            },
        )
        .with_style(Style {
            align_self: AlignSelf::End,
            ..default()
        }),
    );
    // the balls are server-authoritative
    const NUM_BALLS: usize = 0;
    for i in 0..NUM_BALLS {
        let angle: f32 = i as f32 * (TAU / NUM_BALLS as f32);
        let pos = Vec2::new(125.0 * angle.cos(), 125.0 * angle.sin());
        commands.spawn(BallBundle::new(pos, Color::AZURE));
    }
}

/// Read inputs and move players
pub(crate) fn player_movement(mut q: Query<ApplyInputsQuery, With<PlayerId>>) {
    for aiq in q.iter_mut() {
        shared_movement_behaviour(aiq);
    }
}

pub(crate) fn replicate_inputs(
    mut connection: ResMut<ConnectionManager>,
    mut input_events: EventReader<MessageEvent<InputMessage<PlayerActions>>>,
) {
    for event in input_events.read() {
        let inputs = event.message();
        let client_id = event.context();

        // Optional: do some validation on the inputs to check that there's no cheating

        // rebroadcast the input to other clients
        connection
            .send_message_to_target::<InputChannel, _>(
                inputs,
                NetworkTarget::AllExceptSingle(*client_id),
            )
            .unwrap()
    }
}

pub(crate) fn handle_connections(
    mut connections: EventReader<ConnectEvent>,
    mut commands: Commands,
    all_players: Query<Entity, With<PlayerId>>,
) {
    for connection in connections.read() {
        let client_id = connection.client_id;
        info!("New connected client, client_id: {client_id:?}. Spawning player entity..");
        // replicate newly connected clients to all players
        let replicate = Replicate {
            sync: SyncTarget {
                prediction: NetworkTarget::All,
                ..Default::default()
            },
            controlled_by: ControlledBy {
                target: NetworkTarget::Single(client_id),
            },
            // make sure that all entities that are predicted are part of the same replication group
            group: REPLICATION_GROUP,
            ..default()
        };
        // pick color and x,y pos for player
        let player_n = all_players.iter().count();
        let available_colors = [Color::LIME_GREEN, Color::PINK, Color::YELLOW, Color::CYAN];
        let col = available_colors[player_n % available_colors.len()];
        let angle: f32 = player_n as f32 * 5.0;
        let x = 200.0 * angle.cos();
        let y = 200.0 * angle.sin();

        // spawn the player with ActionState - the client will add their own InputMap
        let player_ent = commands
            .spawn((
                PlayerId(client_id),
                Name::new("Player"),
                ActionState::<PlayerActions>::default(),
                Position(Vec2::new(x, y)),
                replicate,
                PhysicsBundle::player_ship(),
                // Rate of fire: 2 bullets per second
                Weapon::new((FIXED_TIMESTEP_HZ / 2.0) as u16),
                // We don't want to replicate the ActionState to the original client, since they are updating it with
                // their own inputs (if you replicate it to the original client, it will be added on the Confirmed entity,
                // which will keep syncing it to the Predicted entity because the ActionState gets updated every tick)!
                OverrideTargetComponent::<ActionState<PlayerActions>>::new(
                    NetworkTarget::AllExceptSingle(client_id),
                ),
                ColorComponent(col),
            ))
            .id();

        info!("Created entity {player_ent:?} for client {client_id:?}");
    }
}
