use crate::protocol::*;
use crate::shared::{color_from_id, shared_config, shared_player_movement};
use crate::{Transports, KEY, PROTOCOL_ID};
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
            incoming_latency: Duration::from_millis(150),
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
                send_diffs_only: false,
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
pub(crate) fn init(mut commands: Commands, mut client: ClientMut, plugin: Res<MyClientPlugin>) {
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
    client.connect();
}

// // The client input only gets applied to predicted entities that we own
// // This works because we only predict the user's controlled entity.
// // If we were predicting more entities, we would have to only apply movement to the player owned one.
// fn player_movement(
//     plugin: Res<MyClientPlugin>,
//     tick_manager: Res<TickManager>,
//     mut player_query: Query<
//         (&mut Transform, &ActionState<PlayerActions>, &PlayerId),
//         With<Predicted>,
//     >,
// ) {
//     for (transform, action_state, player_id) in player_query.iter_mut() {
//         // we only control the movement of our own entity
//         if player_id.0 != plugin.client_id {
//             return;
//         }
//
//         // // TODO: only update if the mouse position has changed
//         // let angle =
//         //     Vec2::new(1.0, 0.0).angle_between(mouse_position - transform.translation.truncate());
//         // transform.rotation = Quat::from_rotation_z(angle);
//         shared_player_movement(transform, action_state);
//         // info!(tick = ?tick_manager.tick(), ?transform, actions = ?action_state.get_pressed(), "applying movement to predicted player");
//     }
// }

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
