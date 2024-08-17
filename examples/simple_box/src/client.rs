//! The client plugin.
//! The client will be responsible for:
//! - connecting to the server at Startup
//! - sending inputs to the server
//! - applying inputs to the locally predicted player (for prediction to work, inputs have to be applied to both the
//! predicted entity and the server entity)
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use bevy::utils::Duration;
use bevy_mod_picking::picking_core::Pickable;
use bevy_mod_picking::prelude::{Click, On, Pointer};
use lightyear::client::input::native::InputSystemSet;
pub use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::Direction;
use crate::protocol::*;
use crate::shared;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_connect_button);
        app.add_systems(
            PreUpdate,
            (handle_connection, handle_disconnection).after(MainSet::Receive),
        );
        // Inputs have to be buffered in the FixedPreUpdate schedule
        app.add_systems(
            FixedPreUpdate,
            buffer_input.in_set(InputSystemSet::BufferInputs),
        );
        app.add_systems(FixedUpdate, player_movement);
        app.add_systems(
            Update,
            (
                receive_message1,
                receive_entity_spawn,
                receive_entity_despawn,
                receive_player_id_insert,
                handle_predicted_spawn,
                handle_interpolated_spawn,
                button_system,
            ),
        );
        app.add_systems(OnEnter(NetworkingState::Disconnected), on_disconnect);
    }
}

/// Component to identify the text displaying the client id
#[derive(Component)]
pub struct ClientIdText;

/// Listen for events to know when the client is connected, and spawn a text entity
/// to display the client id
pub(crate) fn handle_connection(
    mut commands: Commands,
    mut connection_event: EventReader<ConnectEvent>,
) {
    for event in connection_event.read() {
        let client_id = event.client_id();
        commands.spawn((
            TextBundle::from_section(
                format!("Client {}", client_id),
                TextStyle {
                    font_size: 30.0,
                    color: Color::WHITE,
                    ..default()
                },
            ),
            ClientIdText,
        ));
    }
}

/// Listen for events to know when the client is disconnected, and print out the reason
/// of the disconnection
pub(crate) fn handle_disconnection(mut events: EventReader<DisconnectEvent>) {
    for event in events.read() {
        let reason = &event.reason;
        error!("Disconnected from server: {:?}", reason);
    }
}

/// System that reads from peripherals and adds inputs to the buffer
/// This system must be run in the `InputSystemSet::BufferInputs` set in the `FixedPreUpdate` schedule
/// to work correctly.
///
/// I would also advise to use the `leafwing` feature to use the `LeafwingInputPlugin` instead of the
/// `InputPlugin`, which contains more features.
pub(crate) fn buffer_input(
    tick_manager: Res<TickManager>,
    mut input_manager: ResMut<InputManager<Inputs>>,
    keypress: Res<ButtonInput<KeyCode>>,
) {
    let tick = tick_manager.tick();
    let mut input = Inputs::None;
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
        input = Inputs::Direction(direction);
    }
    if keypress.pressed(KeyCode::Backspace) {
        input = Inputs::Delete;
    }
    if keypress.pressed(KeyCode::Space) {
        input = Inputs::Spawn;
    }
    input_manager.add_input(input, tick)
}

/// The client input only gets applied to predicted entities that we own
/// This works because we only predict the user's controlled entity.
/// If we were predicting more entities, we would have to only apply movement to the player owned one.
fn player_movement(
    mut position_query: Query<&mut PlayerPosition, With<Predicted>>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
) {
    for input in input_reader.read() {
        if let Some(input) = input.input() {
            //No need to iterate the position when the input is None
            if (input == &Inputs::None) {
                continue;
            }
            for position in position_query.iter_mut() {
                shared::shared_movement_behaviour(position, input);
            }
        }
    }
}

/// System to receive messages on the client
pub(crate) fn receive_message1(mut reader: EventReader<MessageEvent<Message1>>) {
    for event in reader.read() {
        info!("Received message: {:?}", event.message());
    }
}

/// Example system to handle EntitySpawn events
pub(crate) fn receive_entity_spawn(mut reader: EventReader<EntitySpawnEvent>) {
    for event in reader.read() {
        info!("Received entity spawn: {:?}", event.entity());
    }
}

/// Example system to handle EntitySpawn events
pub(crate) fn receive_entity_despawn(mut reader: EventReader<EntityDespawnEvent>) {
    for event in reader.read() {
        info!("Received entity despawn: {:?}", event.entity());
    }
}

/// Example system to handle ComponentInsertEvent events
pub(crate) fn receive_player_id_insert(mut reader: EventReader<ComponentInsertEvent<PlayerId>>) {
    for event in reader.read() {
        info!(
            "Received component PlayerId insert for entity: {:?}",
            event.entity()
        );
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_predicted_spawn(mut predicted: Query<&mut PlayerColor, Added<Predicted>>) {
    for mut color in predicted.iter_mut() {
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

/// When the predicted copy of the client-owned entity is spawned, do stuff
/// - assign it a different saturation
/// - keep track of it in the Global resource
pub(crate) fn handle_interpolated_spawn(
    mut interpolated: Query<&mut PlayerColor, Added<Interpolated>>,
) {
    for mut color in interpolated.iter_mut() {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
    }
}

/// Create a button that allow you to connect/disconnect to a server
pub(crate) fn spawn_connect_button(mut commands: Commands) {
    commands
        .spawn((
            NodeBundle {
                style: Style {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    align_items: AlignItems::FlexEnd,
                    justify_content: JustifyContent::FlexEnd,
                    flex_direction: FlexDirection::Row,
                    ..default()
                },
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|parent| {
            parent
                .spawn((
                    ButtonBundle {
                        style: Style {
                            width: Val::Px(150.0),
                            height: Val::Px(65.0),
                            border: UiRect::all(Val::Px(5.0)),
                            // horizontally center child text
                            justify_content: JustifyContent::Center,
                            // vertically center child text
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        border_color: BorderColor(Color::BLACK),
                        image: UiImage::default().with_color(Color::srgb(0.15, 0.15, 0.15)),
                        ..default()
                    },
                    On::<Pointer<Click>>::run(|| {}),
                ))
                .with_children(|parent| {
                    parent.spawn((
                        TextBundle::from_section(
                            "Connect",
                            TextStyle {
                                font_size: 20.0,
                                color: Color::srgb(0.9, 0.9, 0.9),
                                ..default()
                            },
                        ),
                        Pickable::IGNORE,
                    ));
                });
        });
}

/// Remove all entities when the client disconnect
fn on_disconnect(
    mut commands: Commands,
    player_entities: Query<Entity, With<PlayerId>>,
    debug_text: Query<Entity, With<ClientIdText>>,
) {
    for entity in player_entities.iter() {
        commands.entity(entity).despawn_recursive();
    }
    for entity in debug_text.iter() {
        commands.entity(entity).despawn_recursive();
    }
}

///  System that will assign a callback to the 'Connect' button depending on the connection state.
fn button_system(
    mut interaction_query: Query<(Entity, &Children, &mut On<Pointer<Click>>), With<Button>>,
    mut text_query: Query<&mut Text>,
    state: Res<State<NetworkingState>>,
) {
    if state.is_changed() {
        for (entity, children, mut on_click) in &mut interaction_query {
            let mut text = text_query.get_mut(children[0]).unwrap();
            match state.get() {
                NetworkingState::Disconnected => {
                    text.sections[0].value = "Connect".to_string();
                    *on_click = On::<Pointer<Click>>::run(|mut commands: Commands| {
                        commands.connect_client();
                    });
                }
                NetworkingState::Connecting => {
                    text.sections[0].value = "Connecting".to_string();
                    *on_click = On::<Pointer<Click>>::run(|| {});
                }
                NetworkingState::Connected => {
                    text.sections[0].value = "Disconnect".to_string();
                    *on_click = On::<Pointer<Click>>::run(|mut commands: Commands| {
                        commands.disconnect_client();
                    });
                }
            };
        }
    }
}
