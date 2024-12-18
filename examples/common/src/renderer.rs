use bevy::prelude::*;
use bevy_mod_picking::picking_core::Pickable;
use bevy_mod_picking::prelude::{Click, On, Pointer};
use bevy_mod_picking::DefaultPickingPlugins;
#[cfg(feature = "bevygap_client")]
use bevygap_client_plugin::prelude::*;
use lightyear::prelude::{client::*, *};

// TODO split into server/client renderer plugins?

pub struct ExampleRendererPlugin {
    /// The name of the example, which must also match the edgegap application name.
    pub name: String,
}

impl ExampleRendererPlugin {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

#[derive(Resource)]
struct GameName(String);

impl Plugin for ExampleRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(DefaultPickingPlugins);
        app.insert_resource(GameName(self.name.clone()));
        app.insert_resource(ClearColor::default());
        // TODO common shortcuts for enabling the egui world inspector etc.
        // TODO handle bevygap ui things.
        // TODO for clients, provide a "connect" button?
        app.add_systems(Startup, (set_window_title, spawn_text));

        #[cfg(feature = "client")]
        {
            #[cfg(feature = "bevygap_client")]
            {
                let bevygap_client_config = BevygapClientConfig {
                    matchmaker_url: crate::settings::get_matchmaker_url(),
                    game_name: self.name.clone(),
                    game_version: "1".to_string(),
                    ..default()
                };
                info!("{bevygap_client_config:?}");
                app.insert_resource(bevygap_client_config);
            }

            app.add_systems(Startup, spawn_connect_button);
            app.add_systems(Update, button_system);
            app.add_systems(
                PreUpdate,
                (handle_connection, handle_disconnection).after(MainSet::Receive),
            );
            app.add_systems(OnEnter(NetworkingState::Disconnected), on_disconnect);

            #[cfg(feature = "client")]
            app.observe(on_update_status_message);
        }

        #[cfg(all(feature = "server", not(feature = "client")))]
        app.add_systems(Startup, spawn_server_text);
    }
}

fn set_window_title(mut window: Query<&mut Window>, game_name: Res<GameName>) {
    let mut window = window.get_single_mut().unwrap();
    window.title = format!("Lightyear Example: {}", game_name.0);
}

fn spawn_text(game_name: Res<GameName>, mut commands: Commands) {
    commands
        .spawn((
            Pickable::IGNORE,
            NodeBundle {
                style: Style {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                ..default()
            },
        ))
        .with_children(|parent| {
            parent
                .spawn(TextBundle::from_section(
                    game_name.0.clone(),
                    TextStyle {
                        font_size: 30.0,
                        color: Color::WHITE.with_alpha(0.05),
                        ..default()
                    },
                ))
                .insert(Style {
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                });
        });
}

/// Spawns a text element that displays "Server"
#[cfg(all(feature = "server", not(feature = "client")))]
fn spawn_server_text(mut commands: Commands) {
    commands.spawn(
        TextBundle::from_section(
            "Server",
            TextStyle {
                font_size: 30.0,
                color: Color::WHITE.with_alpha(0.5),
                ..default()
            },
        )
        .with_style(Style {
            align_self: AlignSelf::End,
            ..default()
        }),
    );
}

#[cfg(feature = "client")]
#[derive(Event, Debug)]
pub struct UpdateStatusMessage(pub String);

#[cfg(feature = "client")]
fn on_update_status_message(
    trigger: Trigger<UpdateStatusMessage>,
    mut q: Query<&mut Text, With<StatusMessageMarker>>,
) {
    for mut text in &mut q {
        text.sections[0].value = trigger.event().0.clone();
    }
}

#[cfg(feature = "client")]
#[derive(Component)]
struct StatusMessageMarker;

#[cfg(feature = "client")]
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
                    TextBundle::from_section(
                        "Lightyear Example",
                        TextStyle {
                            font_size: 18.0,
                            color: Color::srgb(0.9, 0.9, 0.9).with_alpha(0.4),
                            ..default()
                        },
                    ),
                    StatusMessageMarker,
                ))
                .insert(Style {
                    padding: UiRect::all(Val::Px(10.0)),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                });
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

#[cfg(feature = "client")]
///  System that will assign a callback to the 'Connect' button depending on the connection state.
fn button_system(
    mut interaction_query: Query<(Entity, &Children, &mut On<Pointer<Click>>), With<Button>>,
    mut text_query: Query<&mut Text>,
    state: Res<State<NetworkingState>>,
) {
    if state.is_changed() {
        for (_entity, children, mut on_click) in &mut interaction_query {
            let mut text = text_query.get_mut(children[0]).unwrap();
            match state.get() {
                NetworkingState::Disconnected => {
                    text.sections[0].value = "Connect".to_string();
                    *on_click = On::<Pointer<Click>>::run(|mut commands: Commands| {
                        #[cfg(feature = "bevygap_client")]
                        commands.bevygap_connect_client();
                        #[cfg(not(feature = "bevygap_client"))]
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

/// Component to identify the text displaying the client id

#[derive(Component)]
pub struct ClientIdText;

#[cfg(feature = "client")]
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

#[cfg(feature = "client")]
/// Listen for events to know when the client is disconnected, and print out the reason
/// of the disconnection
pub(crate) fn handle_disconnection(
    mut events: EventReader<DisconnectEvent>,
    mut commands: Commands,
) {
    for event in events.read() {
        let reason = &event.reason;
        error!("Disconnected from server: {:?}", reason);
        let msg = match reason {
            None => "".to_string(), // clean.
            Some(reason) => format!("Disconnected: {:?}", reason),
        };
        commands.trigger(UpdateStatusMessage(msg));
    }
}

/// Remove the debug text when the client disconnect
/// (Replicated entities are automatically despawned by lightyear on disconnection)
#[cfg(feature = "client")]
fn on_disconnect(mut commands: Commands, debug_text: Query<Entity, With<ClientIdText>>) {
    for entity in debug_text.iter() {
        commands.entity(entity).despawn_recursive();
    }
}
