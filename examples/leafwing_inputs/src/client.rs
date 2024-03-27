use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use bevy::app::PluginGroupBuilder;
use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
use bevy::prelude::*;
use bevy::utils::Duration;
use bevy_xpbd_2d::parry::shape::ShapeType::Ball;
use bevy_xpbd_2d::prelude::*;
use leafwing_input_manager::prelude::*;

use lightyear::inputs::native::input_buffer::InputBuffer;
use lightyear::prelude::client::LeafwingInputPlugin;
pub use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared::{color_from_id, shared_config, shared_movement_behaviour, FixedSet};
use crate::{shared, ClientTransports, SharedSettings};

pub const INPUT_DELAY_TICKS: u16 = 0;
pub const CORRECTION_TICKS_FACTOR: f32 = 1.5;

pub struct ClientPluginGroup {
    lightyear: ClientPlugin<MyProtocol>,
}

impl ClientPluginGroup {
    pub(crate) fn new(net_config: NetConfig) -> ClientPluginGroup {
        let config = ClientConfig {
            shared: shared_config(),
            net: net_config,
            prediction: PredictionConfig {
                input_delay_ticks: INPUT_DELAY_TICKS,
                correction_ticks_factor: CORRECTION_TICKS_FACTOR,
                ..default()
            },
            interpolation: InterpolationConfig::default()
                .with_delay(InterpolationDelay::default().with_send_interval_ratio(2.0)),
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
            .add(LeafwingInputPlugin::<MyProtocol, PlayerActions>::new(
                LeafwingInputConfig::<PlayerActions> {
                    send_diffs_only: true,
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

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        // To send global inputs, insert the ActionState and the InputMap as Resources
        app.init_resource::<ActionState<AdminActions>>();
        app.insert_resource(InputMap::<AdminActions>::new([
            (AdminActions::SendMessage, KeyCode::KeyM),
            (AdminActions::Reset, KeyCode::KeyR),
        ]));

        app.add_systems(Startup, init);
        app.add_systems(
            PreUpdate,
            spawn_player
                .after(MainSet::ReceiveFlush)
                .before(PredictionSet::SpawnPrediction),
        );
        // all actions related-system that can be rolled back should be in FixedUpdate schedule
        app.add_systems(FixedUpdate, player_movement.in_set(FixedSet::Main));
        app.add_systems(
            Update,
            (
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
pub(crate) fn init(mut commands: Commands, mut client: ResMut<ClientConnection>) {
    commands.spawn(Camera2dBundle::default());
    let _ = client.connect();
}

fn spawn_player(mut commands: Commands, metadata: Res<GlobalMetadata>) {
    // TODO: instead of this, maybe emit an event?
    // the `GlobalMetadata` resource holds metadata related to the client
    // once the connection is established.
    if metadata.is_changed() {
        if let Some(client_id) = metadata.client_id {
            commands.spawn(
                TextBundle::from_section(
                    format!("Client {}", client_id),
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
            let y = (client_id as f32 * 50.0) % 500.0 - 250.0;
            // we will spawn two cubes per player, once is controlled with WASD, the other with arrows
            // if plugin.client_id == 2 {
            commands.spawn(PlayerBundle::new(
                client_id,
                Vec2::new(-50.0, y),
                color_from_id(client_id),
                InputMap::new([
                    (PlayerActions::Up, KeyCode::KeyW),
                    (PlayerActions::Down, KeyCode::KeyS),
                    (PlayerActions::Left, KeyCode::KeyA),
                    (PlayerActions::Right, KeyCode::KeyD),
                ]),
            ));
            // }
            commands.spawn(PlayerBundle::new(
                client_id,
                Vec2::new(50.0, y),
                color_from_id(client_id),
                InputMap::new([
                    (PlayerActions::Up, KeyCode::ArrowUp),
                    (PlayerActions::Down, KeyCode::ArrowDown),
                    (PlayerActions::Left, KeyCode::ArrowLeft),
                    (PlayerActions::Right, KeyCode::ArrowRight),
                ]),
            ));
        }
    }
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

/// When we receive other players (whether they are predicted or interpolated), we want to add the physics components
/// so that our predicted entities can predict collisions with them correctly
fn add_player_physics(
    metadata: Res<GlobalMetadata>,
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
    let Some(client_id) = metadata.client_id else {
        return;
    };

    for (entity, player_id) in player_query.iter_mut() {
        if player_id.0 == client_id {
            // only need to do this for other players' entities
            debug!(
                ?entity,
                ?player_id,
                "we only want to add physics to other player! Skip."
            );
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
        // note that we also apply the input to the other predicted clients! even though
        //  their inputs are only replicated with a delay!
        // TODO: add input decay?
        shared_movement_behaviour(velocity, action_state);
        info!(?entity, tick = ?tick_manager.tick(), ?position, actions = ?action_state.get_pressed(), "applying movement to predicted player");
    }
}

// System to send messages on the client
pub(crate) fn send_message(action_state: Res<ActionState<AdminActions>>) {
    if action_state.just_pressed(&AdminActions::SendMessage) {
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
