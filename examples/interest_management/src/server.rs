use crate::protocol::*;
use crate::shared::{shared_config, shared_movement_behaviour};
use crate::{shared, Transports, KEY, PROTOCOL_ID};
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::shared::replication::components::ReplicationMode;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

pub struct MyServerPlugin {
    pub(crate) transport_config: TransportConfig,
}

const GRID_SIZE: f32 = 200.0;
const NUM_CIRCLES: i32 = 10;
const INTEREST_RADIUS: f32 = 200.0;

// Special room for the player entities (so that all player entities always see each other)
const PLAYER_ROOM: RoomId = RoomId(6000);

pub(crate) async fn create_plugin(port: u16, transport: Transports) -> MyServerPlugin {
    let server_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port);
    let transport_config = match transport {
        Transports::Udp => TransportConfig::UdpSocket(server_addr),
        Transports::WebTransport => {
            let certificate =
                Certificate::load("../certificates/cert.pem", "../certificates/key.pem")
                    .await
                    .unwrap();
            // let certificate = Certificate::self_signed(&["localhost", "127.0.0.1:1334"]);
            let digest = utils::digest_certificate(&certificate);
            dbg!("hashes: {}", certificate.hashes());
            println!("hashes: {}", certificate.hashes().first().unwrap());
            error!(
                "Generated self-signed certificate with digest: {:?}",
                digest
            );
            dbg!(
                "Generated self-signed certificate with digest: {:?}",
                digest
            );
            TransportConfig::WebTransportServer {
                server_addr,
                certificate,
            }
        }
    };
    MyServerPlugin { transport_config }
}

impl Plugin for MyServerPlugin {
    fn build(&self, app: &mut App) {
        let netcode_config = NetcodeConfig::default()
            .with_protocol_id(PROTOCOL_ID)
            .with_key(KEY);
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(100),
            incoming_jitter: Duration::from_millis(10),
            incoming_loss: 0.00,
        };
        let io = Io::from_config(
            IoConfig::from_transport(self.transport_config.clone())
                .with_conditioner(link_conditioner),
        );
        let config = ServerConfig {
            shared: shared_config().clone(),
            netcode: netcode_config,
            ping: PingConfig::default(),
        };
        let plugin_config = PluginConfig::new(config, io, protocol());
        app.add_plugins(ServerPlugin::new(plugin_config));
        app.add_plugins(shared::SharedPlugin);
        app.init_resource::<Global>();
        app.add_systems(Startup, init);
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_plugins(LeafwingInputPlugin::<MyProtocol, Inputs>::default());
        app.add_systems(FixedUpdate, movement.in_set(FixedUpdateSet::Main));
        // input system
        app.add_systems(
            Update,
            (
                handle_connections,
                interest_management,
                log,
                receive_message,
            ),
        );
    }
}

mod utils {
    use super::Certificate;
    use ring::digest::digest;
    use ring::digest::SHA256;

    // Generate a hex-encoded hash of the certificate
    pub fn digest_certificate(certificate: &Certificate) -> String {
        assert_eq!(certificate.certificates().len(), 1);
        certificate
            .certificates()
            .iter()
            .map(|cert| digest(&SHA256, cert).as_ref().to_vec())
            .next()
            .unwrap()
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect::<Vec<_>>()
            .join("")
    }
}

#[derive(Resource, Default)]
pub(crate) struct Global {
    pub client_id_to_entity_id: HashMap<ClientId, Entity>,
    pub client_id_to_room_id: HashMap<ClientId, RoomId>,
}

pub(crate) fn init(mut commands: Commands) {
    commands.spawn(Camera2dBundle::default());
    commands.spawn(TextBundle::from_section(
        "Server",
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
    ));

    // spawn dots in a grid
    for x in -NUM_CIRCLES..NUM_CIRCLES {
        for y in -NUM_CIRCLES..NUM_CIRCLES {
            commands.spawn((
                Position(Vec2::new(x as f32 * GRID_SIZE, y as f32 * GRID_SIZE)),
                Circle,
                Replicate {
                    // use rooms for replication
                    replication_mode: ReplicationMode::Room,
                    ..default()
                },
            ));
        }
    }
}

/// Server connection system, create a player upon connection
pub(crate) fn handle_connections(
    mut server: ResMut<Server>,
    mut connections: EventReader<ConnectEvent>,
    mut disconnections: EventReader<DisconnectEvent>,
    mut global: ResMut<Global>,
    mut commands: Commands,
) {
    for connection in connections.read() {
        let client_id = connection.context();
        // Generate pseudo random color from client id.
        let h = (((client_id * 30) % 360) as f32) / 360.0;
        let s = 0.8;
        let l = 0.5;
        let entity = commands.spawn(PlayerBundle::new(
            *client_id,
            Vec2::ZERO,
            Color::hsl(h, s, l),
        ));
        // Add a mapping from client id to entity id (so that when we receive an input from a client,
        // we know which entity to move)
        global
            .client_id_to_entity_id
            .insert(*client_id, entity.id());
        // we will create a room for each client. To keep things simple, the room id will be the client id
        let room_id = RoomId((*client_id) as u16);
        server.room_mut(room_id).add_client(*client_id);
        server.room_mut(PLAYER_ROOM).add_client(*client_id);
        // also add the player entity to that room (so that the client can always see their own player)
        server.room_mut(room_id).add_entity(entity.id());
        server.room_mut(PLAYER_ROOM).add_entity(entity.id());
    }
    for disconnection in disconnections.read() {
        let client_id = disconnection.context();
        if let Some(entity) = global.client_id_to_entity_id.remove(client_id) {
            commands.entity(entity).despawn();
        }
    }
}

pub(crate) fn log(server: Res<Server>, position: Query<&Position, With<PlayerId>>) {
    let server_tick = server.tick();
    for pos in position.iter() {
        debug!(?server_tick, "Confirmed position: {:?}", pos);
    }
}

pub(crate) fn receive_message(mut messages: EventReader<MessageEvent<Message1>>) {
    for message in messages.read() {
        info!("recv message");
    }
}

/// This is where we perform scope management:
/// - we will add/remove other entities from the player's room only if they are close
pub(crate) fn interest_management(
    mut server: ResMut<Server>,
    player_query: Query<(&PlayerId, Ref<Position>), Without<Circle>>,
    circle_query: Query<(Entity, &Position), With<Circle>>,
) {
    for (client_id, position) in player_query.iter() {
        if position.is_changed() {
            let room_id = RoomId((client_id.0) as u16);
            // let circles_in_room = server.room(room_id).entities();
            let mut room = server.room_mut(room_id);
            for (circle_entity, circle_position) in circle_query.iter() {
                let distance = position.distance(**circle_position);
                if distance < INTEREST_RADIUS {
                    // add the circle to the player's room
                    room.add_entity(circle_entity)
                } else {
                    // if circles_in_room.contains(&circle_entity) {
                    room.remove_entity(circle_entity);
                    // }
                }
            }
        }
    }
}

/// Read client inputs and move players
pub(crate) fn movement(mut position_query: Query<(&mut Position, &ActionState<Inputs>)>) {
    for (position, input) in position_query.iter_mut() {
        shared_movement_behaviour(position, input);
    }
}
