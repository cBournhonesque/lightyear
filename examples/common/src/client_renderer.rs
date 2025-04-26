use bevy::ecs::{relationship::RelatedSpawner, spawn::SpawnWith};
use bevy::picking::prelude::{Click, Pointer};
use bevy::{input::common_conditions::input_toggle_active, prelude::*};
use bevy_inspector_egui::quick::WorldInspectorPlugin;
#[cfg(feature = "bevygap_client")]
use bevygap_client_plugin::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::MainSet;

pub struct ExampleClientRendererPlugin {
    /// The name of the example, which must also match the edgegap application name.
    pub name: String,
}

impl ExampleClientRendererPlugin {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

#[derive(Resource)]
struct GameName(String);

impl Plugin for ExampleClientRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            bevy_inspector_egui::bevy_egui::EguiPlugin {
                enable_multipass_for_primary_context: true,
            },
            // Show inspector with F12
            WorldInspectorPlugin::default().run_if(input_toggle_active(false, KeyCode::F12)),
        ));
        app.insert_resource(GameName(self.name.clone()));
        app.insert_resource(ClearColor::default());
        // TODO common shortcuts for enabling the egui world inspector etc.
        // TODO handle bevygap ui things.
        app.add_systems(Startup, set_window_title);

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

        // spawn the Example label and connect button
        app.world_mut().spawn(name_and_button_bar());

        app.add_systems(
            PreUpdate,
            (handle_connection, handle_disconnection).after(MainSet::Receive),
        );
        app.add_systems(OnEnter(NetworkingState::Disconnected), on_disconnect);

        app.add_systems(Update, update_button_text);
        app.add_observer(on_update_status_message);
    }
}

fn set_window_title(mut window: Query<&mut Window>, game_name: Res<GameName>) {
    let mut window = window.single_mut().unwrap();
    window.title = format!("Lightyear Example: {}", game_name.0);
}

#[derive(Event, Debug)]
pub struct UpdateStatusMessage(pub String);

fn on_update_status_message(
    trigger: Trigger<UpdateStatusMessage>,
    mut q: Query<&mut Text, With<StatusMessageMarker>>,
) {
    for mut text in &mut q {
        text.0 = trigger.event().0.clone();
    }
}

#[derive(Component)]
struct StatusMessageMarker;

fn name_and_button_bar() -> impl Bundle {
    (
        Name::new("Name and Button Bar"),
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            align_items: AlignItems::FlexEnd,
            justify_content: JustifyContent::FlexEnd,
            flex_direction: FlexDirection::Row,
            ..default()
        },
        Children::spawn(SpawnWith(|parent: &mut RelatedSpawner<ChildOf>| {
            parent.spawn((
                Name::new("Example Name"),
                Text("Lightyear Example".to_string()),
                TextColor(Color::srgb(0.9, 0.9, 0.9).with_alpha(0.4)),
                TextFont::from_font_size(18.0),
                StatusMessageMarker,
                Node {
                    padding: UiRect::all(Val::Px(10.0)),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
            ));
            parent
                .spawn(button("Connect"))
                .observe(
                    |_: Trigger<Pointer<Click>>,
                     mut commands: Commands,
                     state: Res<State<NetworkingState>>| {
                        match state.get() {
                            NetworkingState::Disconnected => {
                                #[cfg(feature = "bevygap_client")]
                                commands.bevygap_connect_client();
                                #[cfg(not(feature = "bevygap_client"))]
                                commands.connect_client();
                            }
                            NetworkingState::Connecting | NetworkingState::Connected => {
                                commands.disconnect_client();
                            }
                            _ => {}
                        };
                    },
                );
        })),
    )
}

/// A button bundle
fn button<T: Into<String>>(text: T) -> impl Bundle {
    (
        Button,
        Name::new("Button"),
        BorderColor(Color::WHITE),
        BorderRadius::MAX,
        Node {
            width: Val::Px(150.0),
            height: Val::Px(65.0),
            border: UiRect::all(Val::Px(5.0)),
            // horizontally center child text
            justify_content: JustifyContent::Center,
            // vertically center child text
            align_items: AlignItems::Center,
            ..default()
        },
        Text(text.into()),
        TextColor(Color::srgb(0.9, 0.9, 0.9)),
        TextFont::from_font_size(20.0),
    )
}

pub(crate) fn update_button_text(
    state: Res<State<NetworkingState>>,
    mut text_query: Query<&mut Text, With<Button>>,
) {
    if let Ok(mut text) = text_query.single_mut() {
        match state.get() {
            NetworkingState::Disconnected => {
                text.0 = "Connect".to_string();
            }
            NetworkingState::Connecting => {
                text.0 = "Connecting".to_string();
            }
            NetworkingState::Connected => {
                text.0 = "Disconnect".to_string();
            }
            _ => {}
        };
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
            Text(format!("Client {}", client_id)),
            TextFont::from_font_size(30.0),
            ClientIdText,
        ));
    }
}

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
fn on_disconnect(mut commands: Commands, debug_text: Query<Entity, With<ClientIdText>>) {
    for entity in debug_text.iter() {
        commands.entity(entity).despawn();
    }
}
