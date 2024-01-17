use crate::protocol::*;
use crate::shared::{color_from_id, shared_config, shared_movement_behaviour};
use crate::{Transports, KEY, PROTOCOL_ID};
use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
use bevy::prelude::*;
use bevy_xpbd_3d::parry::shape::ShapeType::Ball;
use bevy_xpbd_3d::prelude::*;
use leafwing_input_manager::action_state::ActionDiff;
use leafwing_input_manager::prelude::*;
use lightyear::inputs::native::input_buffer::InputBuffer;
use lightyear::prelude::client::LeafwingInputPlugin;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

pub const INPUT_DELAY_TICKS: u16 = 20;
pub const CORRECTION_TICKS_FACTOR: f32 = 1.5;

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
            incoming_latency: Duration::from_millis(75),
            incoming_jitter: Duration::from_millis(10),
            incoming_loss: 0.02,
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
            prediction: PredictionConfig {
                input_delay_ticks: INPUT_DELAY_TICKS,
                correction_ticks_factor: CORRECTION_TICKS_FACTOR,
                ..default()
            },
            // we are sending updates every frame (60fps), let's add a delay of 6 network-ticks
            interpolation: InterpolationConfig::default()
                .with_delay(InterpolationDelay::default().with_send_interval_ratio(2.0)),
        };
        let plugin_config = PluginConfig::new(config, io, protocol(), auth);
        app.add_plugins(ClientPlugin::new(plugin_config));
        app.add_plugins(crate::shared::SharedPlugin);
        // add leafwing input plugins, to handle synchronizing leafwing action states correctly
        app.add_plugins(LeafwingInputPlugin::<MyProtocol, PlayerActions>::new(
            LeafwingInputConfig::<PlayerActions> {
                send_diffs_only: true,
                ..default()
            },
        ));
        app.add_plugins(LeafwingInputPlugin::<MyProtocol, AdminActions>::new(
            LeafwingInputConfig::<AdminActions> {
                send_diffs_only: true,
                ..default()
            },
        ));
        // To send global inputs, insert the ActionState and the InputMap as Resources
        app.init_resource::<ActionState<AdminActions>>();
        app.insert_resource(InputMap::<AdminActions>::new([
            (KeyCode::M, AdminActions::SendMessage),
            (KeyCode::R, AdminActions::Reset),
        ]));

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
                spawn_player_mesh,
                add_ball_physics,
                add_player_physics,
                send_message,
                handle_predicted_spawn,
                handle_interpolated_spawn,
            ),
        );
    }
}

// Startup system for the client
pub(crate) fn init(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut client: ClientMut,
    plugin: Res<MyClientPlugin>,
) {
    commands.spawn(Camera3dBundle {
        transform: Transform::from_xyz(0.0, 20.0, 0.5)
            .looking_at(Vec3::ZERO, Vec3::Y),
        ..default()
    });
    commands.spawn(
        TextBundle::from_section(
            format!("Client {}", plugin.client_id),
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
    let x = plugin.client_id as f32 * 3.0;
    // we will spawn two cubes per player, once is controlled with WASD, the other with arrows
    //if plugin.client_id == 2 {
        commands.spawn((
            Name::new(format!("LocalPlayer {}", plugin.client_id)),
            PlayerBundle::new(
                plugin.client_id,
                Vec3::new(-5.0 + x, 2.0, 0.0),
                color_from_id(plugin.client_id),
                InputMap::new([
                    (KeyCode::W, PlayerActions::Up),
                    (KeyCode::S, PlayerActions::Down),
                    (KeyCode::A, PlayerActions::Left),
                    (KeyCode::D, PlayerActions::Right),
                ]),
            ),
            TransformBundle::default(),
            meshes.add(Mesh::from(shape::Cube { size: PLAYER_SIZE })),
            materials.add(Color::rgb(0.8, 0.7, 0.6).into()),
            VisibilityBundle::default(),
        ));
    //}
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
fn add_ball_physics(
    mut commands: Commands,
    mut ball_query: Query<
        Entity,
        (
            With<BallMarker>,
            // insert the physics components on the ball that is displayed on screen
            // (either interpolated or predicted)
            Or<(Added<Interpolated>, Added<Predicted>)>,
        ),
    >,
) {
    for entity in ball_query.iter_mut() {
        commands.entity(entity).insert(PhysicsBundle::ball());
    }
}

fn spawn_player_mesh(
    local_player: Res<MyClientPlugin>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    new_player_query: Query<(Entity, &PlayerId, &ColorComponent), (Without<Handle<Mesh>>, Added<RigidBody>, With<PlayerId>)>,
) {
    for (player_entity, player_id, color) in new_player_query.iter() {
        info!("spawn / draw new Palyer {}", player_id.0 );
        if local_player.client_id == player_id.0 {
            // don't spawn a mesh for the local player
            continue;
        }
        commands.entity(player_entity).insert((
            Name::new(format!("Remote Player {}", player_id.0)),
            PbrBundle {
                mesh: meshes.add(Mesh::from(shape::Cube { size: PLAYER_SIZE })),
                material: materials.add(color.0.into()),
                ..Default::default()
            },
        ));
    }
}

/// When we receive other players (whether they are predicted or interpolated), we want to add the physics components
/// so that our predicted entities can predict collisions with them correctly
fn add_player_physics(
    plugin: Res<MyClientPlugin>,
    mut commands: Commands,
    mut player_query: Query<
        (Entity, &PlayerId),
        (
            // insert the physics components on the player that is displayed on screen
            // (either interpolated or predicted)
            Or<(Added<Interpolated>, Added<Predicted>)>,
        ),
    >,
) {
    for (entity, player_id) in player_query.iter_mut() {
        if player_id.0 == plugin.client_id {
            // only need to do this for other players' entities
            continue;
        }
        info!(?entity, ?player_id, "adding physics to predicted player");
        commands.entity(entity).insert(PhysicsBundle::player());
    }
}

// The client input only gets applied to predicted entities that we own
// This works because we only predict the user's controlled entity.
// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    plugin: Res<MyClientPlugin>,
    tick_manager: Res<TickManager>,
    mut velocity_query: Query<
        (
            Entity,
            &PlayerId,
            &Position,
            &mut LinearVelocity,
            &ActionState<PlayerActions>,
        ),
        With<Predicted>,
    >,
) {
    for (entity, player_id, position, velocity, action_state) in velocity_query.iter_mut() {
        // note that we also apply the input to the other predicted clients!
        // TODO: add input decay?
        shared_movement_behaviour(velocity, action_state);
        info!(?entity, tick = ?tick_manager.tick(), ?position, actions = ?action_state.get_pressed(), "applying movement to predicted player");
    }
}

// System to send messages on the client
pub(crate) fn send_message(action_state: Res<ActionState<AdminActions>>) {
    if action_state.just_pressed(AdminActions::SendMessage) {
        info!("Send message");
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
