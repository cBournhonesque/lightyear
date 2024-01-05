use crate::protocol::*;
use crate::shared::{color_from_id, shared_config, shared_movement_behaviour};
use crate::{Transports, KEY, PROTOCOL_ID};
use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
use bevy::prelude::*;
use bevy_xpbd_2d::parry::shape::ShapeType::Ball;
use bevy_xpbd_2d::prelude::*;
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
            incoming_latency: Duration::from_millis(100),
            incoming_jitter: Duration::from_millis(0),
            incoming_loss: 0.0,
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
        // app.add_plugins(LeafwingInputPlugin::<MyProtocol, AdminActions>::default());

        // We can modify the reporting strategy for system execution order ambiguities on a per-schedule basis
        // app.edit_schedule(PreUpdate, |schedule| {
        //     schedule.set_build_settings(ScheduleBuildSettings {
        //         ambiguity_detection: LogLevel::Warn,
        //         ..default()
        //     });
        // });

        app.insert_resource(self.clone());
        app.add_systems(Startup, init);
        // all actions related-system that can be rolled back should be in FixedUpdateSet::Main
        app.add_systems(
            FixedUpdate,
            player_movement
                .in_set(FixedUpdateSet::Main)
                .before(PhysicsSet::Prepare),
        );
        app.add_systems(
            Update,
            (
                spawn_ball,
                receive_message,
                handle_predicted_spawn,
                handle_interpolated_spawn,
            ),
        );
        // NOTE: on the client we are doing interpolation at PostUpdate time, so we need to sync the physics components
        //  to transform again after that
        // app.add_plugins(
        //     PostUpdate,
        //     (sync_physics_to_transform).in_set(PostUpdateSet::Main),
        // );
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
        Vec2::new(-50.0, 0.0),
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
        Vec2::new(50.0, 0.0),
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

/// Blueprint pattern: when the ball gets replicated from the server, add all the components
/// that we need that are not replicated.
/// (for example physical properties that are constant, so they don't need to be networked)
///
/// We only add the physical properties on the ball that is displayed on screen (i.e the Interpolated ball)
/// We want the ball to be rigid so that when players collide with it, they bounce off.
///
/// However we remove the Position because we want the balls position to be interpolated, without being computed/updated
/// by the physics engine? Actually this shouldn't matter because we run interpolation in PostUpdate...
fn spawn_ball(
    mut commands: Commands,
    mut ball_query: Query<Entity, (With<BallMarker>, Added<Interpolated>)>,
) {
    for entity in ball_query.iter_mut() {
        commands.entity(entity).insert(PhysicsBundle::ball());
        commands
            .entity(entity)
            .remove::<(Position, LinearVelocity)>();
    }
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    client: Res<Client>,
    mut velocity_query: Query<
        (&Position, &mut LinearVelocity, &ActionState<PlayerActions>),
        With<Predicted>,
    >,
    // mut velocity_query: Query<
    //     (&Transform, &mut LinearVelocity, &ActionState<PlayerActions>),
    //     With<Predicted>,
    // >,
) {
    for (position, velocity, action_state) in velocity_query.iter_mut() {
        shared_movement_behaviour(client.tick(), position, velocity, action_state);
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
