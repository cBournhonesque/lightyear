use crate::protocol::{Direction, Inputs, Message1, MyProtocol, PlayerPosition};
use crate::shared::{shared_config, shared_movement_behaviour};
use crate::{KEY, PROTOCOL_ID};
use bevy::prelude::*;
use lightyear_shared::client::prediction::Predicted;
use lightyear_shared::client::{Authentication, ClientConfig, InputSystemSet};
use lightyear_shared::plugin::events::{InputEvent, MessageEvent};
use lightyear_shared::plugin::sets::FixedUpdateSet;
use lightyear_shared::{Client, ClientId, EntitySpawnEvent, IoConfig, TransportConfig};
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

pub struct ClientPlugin {
    pub(crate) client_id: ClientId,
    pub(crate) server_port: u16,
}

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), self.server_port);
        let auth = Authentication::Manual {
            server_addr,
            client_id: self.client_id,
            private_key: KEY,
            protocol_id: PROTOCOL_ID,
        };
        let addr = SocketAddr::from_str("127.0.0.1:0").unwrap();
        let config = ClientConfig {
            shared: shared_config().clone(),
            netcode: Default::default(),
            io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
            ping: lightyear_shared::client::PingConfig::default(),
        };
        let plugin_config =
            lightyear_shared::client::PluginConfig::new(config, MyProtocol::default(), auth);
        app.add_plugins(lightyear_shared::client::Plugin::new(plugin_config));
        app.add_plugins(crate::shared::SharedPlugin);
        app.add_systems(Startup, init);
        app.add_systems(
            FixedUpdate,
            buffer_input.in_set(InputSystemSet::BufferInputs),
        );
        app.add_systems(FixedUpdate, movement.in_set(FixedUpdateSet::Main));
        app.add_systems(Update, (receive_message1, receive_entity_spawn));
    }
}

// Startup system for the client
pub(crate) fn init(mut commands: Commands, mut client: ResMut<Client<MyProtocol>>) {
    commands.spawn(Camera2dBundle::default());
    commands.spawn(TextBundle::from_section(
        "Client",
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
    ));
    client.connect();
}

// System that reads from peripherals and adds inputs to the buffer
pub(crate) fn buffer_input(mut client: ResMut<Client<MyProtocol>>, keypress: Res<Input<KeyCode>>) {
    let mut input = Direction {
        up: false,
        down: false,
        left: false,
        right: false,
    };
    if keypress.pressed(KeyCode::W) || keypress.pressed(KeyCode::Up) {
        input.up = true;
    }
    if keypress.pressed(KeyCode::S) || keypress.pressed(KeyCode::Down) {
        input.down = true;
    }
    if keypress.pressed(KeyCode::A) || keypress.pressed(KeyCode::Left) {
        input.left = true;
    }
    if keypress.pressed(KeyCode::D) || keypress.pressed(KeyCode::Right) {
        input.right = true;
    }
    if keypress.pressed(KeyCode::Delete) {
        // currently, inputs is an enum and we can only add one input per tick
        return client.add_input(Inputs::Delete);
    }
    if keypress.pressed(KeyCode::Space) {
        return client.add_input(Inputs::Spawn);
    }
    info!("Sending input: {:?} on tick: {:?}", &input, client.tick());
    client.add_input(Inputs::Direction(input));
}

// The client input only gets applied to predicted entities
pub(crate) fn movement(
    mut position_query: Query<&mut PlayerPosition, With<Predicted>>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
) {
    for input in input_reader.read() {
        if input.input().is_some() {
            let input = input.input().as_ref().unwrap();
            for mut position in position_query.iter_mut() {
                shared_movement_behaviour(&mut position, input);
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

pub(crate) fn receive_entity_spawn(mut reader: EventReader<EntitySpawnEvent>) {
    for event in reader.read() {
        info!("Received entity spawn: {:?}", event.entity());
    }
}
