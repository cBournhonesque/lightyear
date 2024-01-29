use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared::{shared_config, shared_movement_behaviour};
use crate::{Connections, Transports, KEY, PROTOCOL_ID};
use bevy::prelude::*;
use bevy::utils::Duration;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

pub struct MyClientPlugin {
    pub(crate) client_id: ClientId,
    pub(crate) plugin: ClientPlugin<MyProtocol>,
}

/// Resource that keeps track of the client id
#[derive(Resource)]
pub struct ClientIdResource {
    client_id: ClientId,
}

impl MyClientPlugin {
    /// Add all the plugins that make up the Client
    pub(crate) fn build(self, app: &mut App) {
        app.add_plugins(self.plugin);
        app.add_plugins(crate::shared::SharedPlugin);
        app.insert_resource(ClientIdResource {
            client_id: self.client_id,
        });
        app.add_systems(Startup, init);
        app.add_systems(
            FixedUpdate,
            buffer_input.in_set(InputSystemSet::BufferInputs),
        );
        app.add_systems(FixedUpdate, player_movement.in_set(FixedUpdateSet::Main));
        app.add_systems(
            Update,
            (
                receive_message1,
                receive_entity_spawn,
                receive_entity_despawn,
                handle_predicted_spawn,
                handle_interpolated_spawn,
            ),
        );
    }
}

pub(crate) fn create_plugin(
    client_id: u64,
    client_port: u16,
    server_addr: SocketAddr,
    transport: Transports,
    connection: Connections,
) -> MyClientPlugin {
    let auth = Authentication::Manual {
        server_addr,
        client_id,
        private_key: KEY,
        protocol_id: PROTOCOL_ID,
    };
    let client_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), client_port);
    let certificate_digest =
        String::from("6c594425dd0c8664c188a0ad6e641b39ff5f007e5bcfc1e72c7a7f2f38ecf819");
    let transport_config = match transport {
        #[cfg(not(target_family = "wasm"))]
        Transports::Udp => TransportConfig::UdpSocket(client_addr),
        Transports::WebTransport => TransportConfig::WebTransportClient {
            client_addr,
            server_addr,
            #[cfg(target_family = "wasm")]
            certificate_digest,
        },
    };
    let link_conditioner = LinkConditionerConfig {
        incoming_latency: Duration::from_millis(200),
        incoming_jitter: Duration::from_millis(20),
        incoming_loss: 0.05,
    };
    let io = Io::from_config(
        IoConfig::from_transport(transport_config).with_conditioner(link_conditioner),
    );
    let netconfig = match connection {
        Connections::Netcode => NetConfig::Netcode {
            auth,
            config: NetcodeConfig::default(),
        },
        #[cfg(feature = "lightyear/rivet")]
        Connections::Rivet => NetConfig::Rivet {
            config: NetcodeConfig::default(),
        },
    };
    let config = ClientConfig {
        shared: shared_config().clone(),
        net: netconfig,
        ..default()
    };
    let plugin = ClientPlugin::new(PluginConfig::new(config, io, protocol()));
    MyClientPlugin { client_id, plugin }
}

// Startup system for the client
pub(crate) fn init(mut commands: Commands, mut client: ClientMut, plugin: Res<ClientIdResource>) {
    commands.spawn(Camera2dBundle::default());
    commands.spawn(TextBundle::from_section(
        format!("Client {}", plugin.client_id),
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
    ));
    let _ = client.connect();
}

// System that reads from peripherals and adds inputs to the buffer
pub(crate) fn buffer_input(mut client: ClientMut, keypress: Res<Input<KeyCode>>) {
    let mut direction = Direction {
        up: false,
        down: false,
        left: false,
        right: false,
    };
    if keypress.pressed(KeyCode::W) || keypress.pressed(KeyCode::Up) {
        direction.up = true;
    }
    if keypress.pressed(KeyCode::S) || keypress.pressed(KeyCode::Down) {
        direction.down = true;
    }
    if keypress.pressed(KeyCode::A) || keypress.pressed(KeyCode::Left) {
        direction.left = true;
    }
    if keypress.pressed(KeyCode::D) || keypress.pressed(KeyCode::Right) {
        direction.right = true;
    }
    if !direction.is_none() {
        return client.add_input(Inputs::Direction(direction));
    }
    if keypress.pressed(KeyCode::Delete) {
        // currently, inputs is an enum and we can only add one input per tick
        return client.add_input(Inputs::Delete);
    }
    if keypress.pressed(KeyCode::Space) {
        return client.add_input(Inputs::Spawn);
    }
    // info!("Sending input: {:?} on tick: {:?}", &input, client.tick());
    return client.add_input(Inputs::None);
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    // TODO: maybe make prediction mode a separate component!!!
    mut position_query: Query<&mut PlayerPosition, With<Predicted>>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
) {
    if <Components as SyncMetadata<PlayerPosition>>::mode() != ComponentSyncMode::Full {
        return;
    }
    for input in input_reader.read() {
        if let Some(input) = input.input() {
            for position in position_query.iter_mut() {
                shared_movement_behaviour(position, input);
            }
        }
    }
}

// System to receive messages on the client
pub(crate) fn receive_message1(mut reader: EventReader<MessageEvent<Message1>>) {
    for event in reader.read() {
        info!("Received message: {:?}", event.message());
    }
}

// Example system to handle EntitySpawn events
pub(crate) fn receive_entity_spawn(mut reader: EventReader<EntitySpawnEvent>) {
    for event in reader.read() {
        info!("Received entity spawn: {:?}", event.entity());
    }
}

// Example system to handle EntitySpawn events
pub(crate) fn receive_entity_despawn(mut reader: EventReader<EntityDespawnEvent>) {
    for event in reader.read() {
        info!("Received entity despawn: {:?}", event.entity());
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(mut predicted: Query<&mut PlayerColor, Added<Predicted>>) {
    for mut color in predicted.iter_mut() {
        color.0.set_s(0.3);
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - keep track of it in the Global resource
pub(crate) fn handle_interpolated_spawn(
    mut interpolated: Query<&mut PlayerColor, Added<Interpolated>>,
) {
    for mut color in interpolated.iter_mut() {
        color.0.set_s(0.1);
    }
}
