use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared::{shared_config, shared_movement_behaviour};
use crate::{shared, Transports, KEY, PROTOCOL_ID};
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use bevy::utils::Duration;
use lightyear::client::resource::connect_with_token;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

pub struct ClientPluginGroup {
    client_id: ClientId,
    lightyear: ClientPlugin<MyProtocol>,
}

impl ClientPluginGroup {
    pub(crate) fn new(
        client_id: u64,
        client_port: u16,
        server_addr: SocketAddr,
        transport: Transports,
    ) -> ClientPluginGroup {
        let auth = Authentication::Manual {
            server_addr,
            client_id,
            private_key: KEY,
            protocol_id: PROTOCOL_ID,
        };
        let client_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), client_port);
        let certificate_digest =
            String::from("2b:08:3b:2a:2b:9a:ad:dc:ed:ba:80:43:c3:1a:43:3e:2c:06:11:a0:61:25:4b:fb:ca:32:0e:5d:85:5d:a7:56")
                .replace(":", "");
        let transport_config = match transport {
            #[cfg(not(target_family = "wasm"))]
            Transports::Udp => TransportConfig::UdpSocket(client_addr),
            Transports::WebTransport => TransportConfig::WebTransportClient {
                client_addr,
                server_addr,
                #[cfg(target_family = "wasm")]
                certificate_digest,
            },
            Transports::WebSocket => TransportConfig::WebSocketClient { server_addr },
        };
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(200),
            incoming_jitter: Duration::from_millis(20),
            incoming_loss: 0.05,
        };
        let config = ClientConfig {
            shared: shared_config(),
            net: NetConfig::Netcode {
                auth,
                config: NetcodeConfig::default(),
                io: IoConfig::from_transport(transport_config).with_conditioner(link_conditioner),
            },
            interpolation: InterpolationConfig {
                delay: InterpolationDelay::default().with_send_interval_ratio(2.0),
                custom_interpolation_logic: false,
            },
            ..default()
        };
        let plugin_config = PluginConfig::new(config, protocol());
        ClientPluginGroup {
            client_id,
            lightyear: ClientPlugin::new(plugin_config),
        }
    }
}

impl PluginGroup for ClientPluginGroup {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(self.lightyear)
            .add(ExampleClientPlugin {
                client_id: self.client_id,
            })
            .add(shared::SharedPlugin)
    }
}

pub struct ExampleClientPlugin {
    client_id: ClientId,
}

#[derive(Resource)]
pub struct Global {
    client_id: ClientId,
}

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Global {
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
        // app.add_systems(Update, connect.run_if(on_timer(Duration::from_secs(10))));
    }
}

// Startup system for the client
pub(crate) fn init(mut commands: Commands, mut client: ClientMut, global: Res<Global>) {
    commands.spawn(Camera2dBundle::default());
    commands.spawn(TextBundle::from_section(
        format!("Client {}", global.client_id),
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
    ));
    let _ = client.connect();
}

// pub(crate) fn connect(world: &mut World) {
//     // if world.get_resource::<Time>().unwrap().elapsed() < Duration::from_secs(3) {
//     //     return;
//     // }
//     let server_addr = SocketAddr::from_str("127.0.0.1:5000").unwrap();
//     let client_port = 0;
//     let client_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), client_port);
//     let certificate_digest =
//         String::from("2b:08:3b:2a:2b:9a:ad:dc:ed:ba:80:43:c3:1a:43:3e:2c:06:11:a0:61:25:4b:fb:ca:32:0e:5d:85:5d:a7:56")
//             .replace(":", "");
//     let transport_config = TransportConfig::WebTransportClient {
//         client_addr,
//         server_addr,
//     };
//     let link_conditioner = LinkConditionerConfig {
//         incoming_latency: Duration::from_millis(200),
//         incoming_jitter: Duration::from_millis(20),
//         incoming_loss: 0.05,
//     };
//     let io = Io::from_config(
//         IoConfig::from_transport(transport_config).with_conditioner(link_conditioner),
//     );
//     info!("CONNECT");
//     let auth = Authentication::Manual {
//         server_addr,
//         client_id: 1,
//         private_key: KEY,
//         protocol_id: PROTOCOL_ID,
//     };
//     let token = auth.get_token(20).unwrap();
//     let _ = connect_with_token(world, token, io);
// }

// System that reads from peripherals and adds inputs to the buffer
pub(crate) fn buffer_input(mut client: ClientMut, keypress: Res<ButtonInput<KeyCode>>) {
    let mut direction = Direction {
        up: false,
        down: false,
        left: false,
        right: false,
    };
    if keypress.pressed(KeyCode::KeyW) || keypress.pressed(KeyCode::ArrowUp) {
        direction.up = true;
    }
    if keypress.pressed(KeyCode::KeyS) || keypress.pressed(KeyCode::ArrowDown) {
        direction.down = true;
    }
    if keypress.pressed(KeyCode::KeyA) || keypress.pressed(KeyCode::ArrowLeft) {
        direction.left = true;
    }
    if keypress.pressed(KeyCode::KeyD) || keypress.pressed(KeyCode::ArrowRight) {
        direction.right = true;
    }
    if !direction.is_none() {
        return client.add_input(Inputs::Direction(direction));
    }
    if keypress.pressed(KeyCode::Backspace) {
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
