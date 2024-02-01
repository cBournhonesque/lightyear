use crate::protocol::*;
use crate::shared::{color_from_id, shared_config, shared_player_movement};
use crate::{shared, Transports, KEY, PROTOCOL_ID};
use bevy::app::PluginGroupBuilder;
use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
use bevy::prelude::*;
use bevy::utils::Duration;
use leafwing_input_manager::action_state::ActionDiff;
use leafwing_input_manager::axislike::DualAxisData;
use leafwing_input_manager::buttonlike::ButtonState::Pressed;
use leafwing_input_manager::orientation::Orientation;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::*;
use lightyear::inputs::native::input_buffer::InputBuffer;
use lightyear::prelude::client::LeafwingInputPlugin;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

pub const INPUT_DELAY_TICKS: u16 = 0;
pub const CORRECTION_TICKS_FACTOR: f32 = 1.5;

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
            String::from("6c594425dd0c8664c188a0ad6e641b39ff5f007e5bcfc1e72c7a7f2f38ecf819")
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
        };
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(150),
            incoming_jitter: Duration::from_millis(10),
            incoming_loss: 0.02,
        };
        let io = Io::from_config(
            IoConfig::from_transport(transport_config).with_conditioner(link_conditioner),
        );
        let config = ClientConfig {
            shared: shared_config(),
            net: NetConfig::Netcode {
                auth,
                config: NetcodeConfig::default(),
            },
            prediction: PredictionConfig {
                input_delay_ticks: INPUT_DELAY_TICKS,
                correction_ticks_factor: CORRECTION_TICKS_FACTOR,
                ..default()
            },
            interpolation: InterpolationConfig::default()
                .with_delay(InterpolationDelay::default().with_send_interval_ratio(2.0)),
            ..default()
        };
        let plugin_config = PluginConfig::new(config, io, protocol());
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
            .add(LeafwingInputPlugin::<MyProtocol, PlayerActions>::new(
                LeafwingInputConfig::<PlayerActions> {
                    send_diffs_only: false,
                    ..default()
                },
            ))
            .add(LeafwingInputPlugin::<MyProtocol, AdminActions>::new(
                LeafwingInputConfig::<AdminActions> {
                    send_diffs_only: true,
                    ..default()
                },
            ))
    }
}

pub struct ExampleClientPlugin {
    client_id: ClientId,
}

#[derive(Resource)]
pub struct ClientIdResource {
    client_id: ClientId,
}

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // To send global inputs, insert the ActionState and the InputMap as Resources
        app.init_resource::<ActionState<AdminActions>>();
        app.insert_resource(InputMap::<AdminActions>::new([
            (KeyCode::M, AdminActions::SendMessage),
            (KeyCode::R, AdminActions::Reset),
        ]));

        app.insert_resource(ClientIdResource {
            client_id: self.client_id,
        });
        app.add_systems(Startup, init);
        // all actions related-system that can be rolled back should be in FixedUpdateSet::Main
        // app.add_systems(FixedUpdate, player_movement.in_set(FixedUpdateSet::Main));
        // we update the ActionState manually from cursor, so we need to put it in the ManualControl set
        app.add_systems(
            PreUpdate,
            update_cursor_state_from_window.in_set(InputManagerSystem::ManualControl),
        );
        app.add_systems(Update, (handle_predicted_spawn, handle_interpolated_spawn));
    }
}

// Startup system for the client
pub(crate) fn init(mut commands: Commands, mut client: ClientMut, plugin: Res<ClientIdResource>) {
    commands.spawn(Camera2dBundle::default());
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
    let y = (plugin.client_id as f32 * 50.0) % 500.0 - 250.0;
    commands.spawn(PlayerBundle::new(
        plugin.client_id,
        Vec2::new(-50.0, y),
        color_from_id(plugin.client_id),
        InputMap::new([
            (KeyCode::W, PlayerActions::Up),
            (KeyCode::S, PlayerActions::Down),
            (KeyCode::A, PlayerActions::Left),
            (KeyCode::D, PlayerActions::Right),
            (KeyCode::Space, PlayerActions::Shoot),
        ]),
    ));
    let _ = client.connect();
}

fn update_cursor_state_from_window(
    window_query: Query<&Window>,
    mut action_state_query: Query<&mut ActionState<PlayerActions>, With<Predicted>>,
) {
    // Update the action-state with the mouse position from the window
    for window in window_query.iter() {
        for mut action_state in action_state_query.iter_mut() {
            if let Some(val) = window_relative_mouse_position(window) {
                action_state
                    .action_data_mut(PlayerActions::MoveCursor)
                    .axis_pair = Some(DualAxisData::from_xy(val));
                action_state
                    .action_data_mut(PlayerActions::MoveCursor)
                    .state = Pressed;
            }
        }
    }
}

// Get the cursor position relative to the window
fn window_relative_mouse_position(window: &Window) -> Option<Vec2> {
    let Some(cursor_pos) = window.cursor_position() else {
        return None;
    };

    Some(Vec2::new(
        cursor_pos.x - (window.width() / 2.0),
        (cursor_pos.y - (window.height() / 2.0)) * -1.0,
    ))
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
