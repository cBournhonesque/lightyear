use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use leafwing_input_manager::plugin::InputManagerSystem;
use leafwing_input_manager::prelude::*;
use leafwing_input_manager::systems::{run_if_enabled, tick_action_state};

use lightyear::_reexport::ShouldBeInterpolated;
pub use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared::shared_config;
use crate::{shared, ClientTransports, SharedSettings};

pub struct ClientPluginGroup {
    lightyear: ClientPlugin<MyProtocol>,
}

impl ClientPluginGroup {
    pub(crate) fn new(net_config: NetConfig) -> ClientPluginGroup {
        let config = ClientConfig {
            shared: shared_config(),
            net: net_config,
            packet: PacketConfig::default()
                // by default there is no bandwidth limit so we need to enable it
                .enable_bandwidth_cap()
                // we can set the max bandwidth to 56 KB/s
                .with_send_bandwidth_bytes_per_second_cap(56000),
            interpolation: InterpolationConfig {
                delay: InterpolationDelay::default().with_send_interval_ratio(2.0),
                custom_interpolation_logic: false,
            },
            ..default()
        };
        let plugin_config = PluginConfig::new(config, protocol());
        ClientPluginGroup {
            lightyear: ClientPlugin::new(plugin_config),
        }
    }
}

impl PluginGroup for ClientPluginGroup {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(self.lightyear)
            .add(ExampleClientPlugin)
            .add(shared::SharedPlugin)
            .add(LeafwingInputPlugin::<MyProtocol, Inputs>::default())
    }
}

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActionState<Inputs>>();
        app.add_systems(Startup, init);
        app.add_systems(
            PreUpdate,
            handle_client_connection.after(MainSet::ReceiveFlush),
        );
        app.add_systems(
            Update,
            (
                add_input_map,
                handle_predicted_spawn,
                handle_interpolated_spawn,
                // log,
            ),
        );
    }
}

// Startup system for the client
pub(crate) fn init(mut commands: Commands, mut client: ResMut<ClientConnection>) {
    commands.spawn(Camera2dBundle::default());
    let _ = client.connect();
}

// System that handles when the client gets connected (we receive the ClientId in the GlobalMetadata resource)
pub(crate) fn handle_client_connection(mut commands: Commands, metadata: Res<GlobalMetadata>) {
    // the `GlobalMetadata` resource holds metadata related to the client
    // once the connection is established.
    if metadata.is_changed() {
        if let Some(client_id) = metadata.client_id {
            commands.spawn(TextBundle::from_section(
                format!("Client {}", client_id),
                TextStyle {
                    font_size: 30.0,
                    color: Color::WHITE,
                    ..default()
                },
            ));
        }
    }
}

// System to receive messages on the client
pub(crate) fn add_input_map(
    mut commands: Commands,
    predicted_players: Query<Entity, (Added<PlayerId>, With<Predicted>)>,
) {
    for player_entity in predicted_players.iter() {
        commands.entity(player_entity).insert((
            PlayerBundle::get_input_map(),
            ActionState::<Inputs>::default(),
        ));
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
pub(crate) fn handle_predicted_spawn(mut predicted: Query<&mut PlayerColor, Added<Predicted>>) {
    for mut color in predicted.iter_mut() {
        color.0.set_s(0.3);
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
pub(crate) fn handle_interpolated_spawn(
    mut interpolated: Query<&mut PlayerColor, Added<Interpolated>>,
) {
    for mut color in interpolated.iter_mut() {
        color.0.set_s(0.1);
    }
}

pub(crate) fn log(
    tick_manager: Res<TickManager>,
    connection: Res<ClientConnectionManager>,
    confirmed: Query<&Position, With<Confirmed>>,
    predicted: Query<&Position, (With<Predicted>, Without<Confirmed>)>,
    mut interp_event: EventReader<ComponentInsertEvent<ShouldBeInterpolated>>,
    mut predict_event: EventReader<ComponentInsertEvent<ShouldBePredicted>>,
) {
    let server_tick = connection.latest_received_server_tick();
    for confirmed_pos in confirmed.iter() {
        debug!(?server_tick, "Confirmed position: {:?}", confirmed_pos);
    }
    let client_tick = tick_manager.tick();
    for predicted_pos in predicted.iter() {
        debug!(?client_tick, "Predicted position: {:?}", predicted_pos);
    }
    for event in interp_event.read() {
        info!("Interpolated event: {:?}", event.entity());
    }
    for event in predict_event.read() {
        info!("Predicted event: {:?}", event.entity());
    }
}
