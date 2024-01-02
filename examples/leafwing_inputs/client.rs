use crate::protocol::*;
use crate::shared::{color_from_id, shared_config, shared_movement_behaviour};
use crate::{Transports, KEY, PROTOCOL_ID};
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::_reexport::ShouldBePredicted;
use lightyear::prelude::client::LeafwingInputPlugin;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

#[derive(Resource, Clone, Copy)]
pub struct MyClientPlugin {
    pub(crate) client_id: ClientId,
    pub(crate) client_port: u16,
    pub(crate) server_addr: Ipv4Addr,
    pub(crate) server_port: u16,
    pub(crate) transport: Transports,
}

impl Plugin for MyClientPlugin {
    fn build(&self, app: &mut App) {
        let server_addr = SocketAddr::new(self.server_addr.into(), self.server_port);
        let auth = Authentication::Manual {
            server_addr,
            client_id: self.client_id,
            private_key: KEY,
            protocol_id: PROTOCOL_ID,
        };
        let client_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), self.client_port);
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(900),
            incoming_jitter: Duration::from_millis(20),
            incoming_loss: 0.05,
        };
        let transport = match self.transport {
            Transports::Udp => TransportConfig::UdpSocket(client_addr),
            Transports::Webtransport => TransportConfig::WebTransportClient {
                client_addr,
                server_addr,
            },
        };
        let io =
            Io::from_config(IoConfig::from_transport(transport).with_conditioner(link_conditioner));
        let config = ClientConfig {
            shared: shared_config().clone(),
            input: InputConfig::default(),
            netcode: Default::default(),
            ping: PingConfig::default(),
            sync: SyncConfig::default(),
            prediction: PredictionConfig::default(),
            // we are sending updates every frame (60fps), let's add a delay of 6 network-ticks
            interpolation: InterpolationConfig::default()
                .with_delay(InterpolationDelay::default().with_send_interval_ratio(2.0)),
        };
        let plugin_config = PluginConfig::new(config, io, protocol(), auth);
        app.add_plugins(ClientPlugin::new(plugin_config));
        app.add_plugins(crate::shared::SharedPlugin);
        // add leafwing input plugins, to handle synchronizing leafwing action states correctly
        app.add_plugins(LeafwingInputPlugin::<MyProtocol, PlayerActions>::default());
        app.add_plugins(LeafwingInputPlugin::<MyProtocol, AdminActions>::default());

        app.insert_resource(self.clone());
        app.add_systems(Startup, init);
        // all actions related-system that can be rolled back should be in FixedUpdateSet::Main
        app.add_systems(FixedUpdate, player_movement.in_set(FixedUpdateSet::Main));
        app.add_systems(
            Update,
            (
                receive_message,
                handle_predicted_spawn,
                handle_interpolated_spawn,
            ),
        );
    }
}

// Startup system for the client
pub(crate) fn init(
    mut commands: Commands,
    mut client: ResMut<Client>,
    plugin: Res<MyClientPlugin>,
) {
    commands.spawn(Camera2dBundle::default());
    commands.spawn(TextBundle::from_section(
        format!("Client {}", plugin.client_id),
        TextStyle {
            font_size: 30.0,
            color: bevy::prelude::Color::WHITE,
            ..default()
        },
    ));
    // we will spawn two cubes per player, once is controlled with WASD, the other with arrows
    let mut entity = commands.spawn(PlayerBundle::new(
        plugin.client_id,
        Vec2::new(-10.0, 0.0),
        color_from_id(plugin.client_id),
        InputMap::new([
            (KeyCode::W, PlayerActions::Up),
            (KeyCode::S, PlayerActions::Down),
            (KeyCode::A, PlayerActions::Left),
            (KeyCode::D, PlayerActions::Right),
        ]),
    ));
    let entity_id = entity.id();
    // IMPORTANT: this lets the server know that the entity is pre-predicted
    // when the server replicates this entity; we will get a Confirmed entity which will use this entity
    // as the Predicted version
    entity.insert(ShouldBePredicted {
        client_entity: Some(entity_id),
    });

    let mut entity = commands.spawn(PlayerBundle::new(
        plugin.client_id,
        Vec2::new(10.0, 0.0),
        color_from_id(plugin.client_id),
        InputMap::new([
            (KeyCode::Up, PlayerActions::Up),
            (KeyCode::Down, PlayerActions::Down),
            (KeyCode::Left, PlayerActions::Left),
            (KeyCode::Right, PlayerActions::Right),
        ]),
    ));
    let entity_id = entity.id();
    // IMPORTANT: this lets the server know that the entity is pre-predicted
    // when the server replicates this entity; we will get a Confirmed entity which will use this entity
    // as the Predicted version
    entity.insert(ShouldBePredicted {
        client_entity: Some(entity_id),
    });
    client.connect();
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    mut position_query: Query<(&mut Position, &ActionState<PlayerActions>), With<Predicted>>,
) {
    if Position::mode() != ComponentSyncMode::Full {
        return;
    }
    for (position, action_state) in position_query.iter_mut() {
        shared_movement_behaviour(position, action_state);
    }
}

// System to receive messages on the client
pub(crate) fn receive_message(mut reader: EventReader<MessageEvent<Message1>>) {
    for event in reader.read() {
        info!("Received message: {:?}", event.message());
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(mut predicted: Query<&mut ColorComponent, Added<Predicted>>) {
    for mut color in predicted.iter_mut() {
        color.0.set_s(0.4);
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - keep track of it in the Global resource
pub(crate) fn handle_interpolated_spawn(
    mut interpolated: Query<&mut ColorComponent, Added<Interpolated>>,
) {
    for mut color in interpolated.iter_mut() {
        color.0.set_s(0.1);
    }
}
