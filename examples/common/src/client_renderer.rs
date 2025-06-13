use bevy::picking::prelude::{Click, Pointer};
use bevy::prelude::*;
#[cfg(feature = "bevygap_client")]
use bevygap_client_plugin::prelude::*;
use lightyear::connection::client::ClientState;
use lightyear::prelude::*;

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
        app.insert_resource(GameName(self.name.clone()));
        app.insert_resource(ClearColor::default());
        // TODO common shortcuts for enabling the egui world inspector etc.
        app.add_systems(Startup, set_window_title);
        spawn_connect_button(app);
        app.add_systems(Update, update_button_text);
        app.add_observer(on_update_status_message);
        app.add_observer(handle_connection);
        app.add_observer(handle_disconnection);
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

/// Create a button that allow you to connect/disconnect to a server
pub(crate) fn spawn_connect_button(app: &mut App) {
    app.world_mut()
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            align_items: AlignItems::FlexEnd,
            justify_content: JustifyContent::FlexEnd,
            flex_direction: FlexDirection::Row,
            ..default()
        })
        .with_children(|parent| {
            parent.spawn((
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
                .spawn((
                    Text("Connect".to_string()),
                    TextColor(Color::srgb(0.9, 0.9, 0.9)),
                    TextFont::from_font_size(20.0),
                    BorderColor(Color::BLACK),
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
                    Button,
                ))
                .observe(
                    |_: Trigger<Pointer<Click>>,
                     mut commands: Commands,
                     query: Query<(Entity, &Client)>| {
                        let Ok((entity, client)) = query.single() else {
                            return;
                        };
                        match client.state {
                            ClientState::Disconnected => {
                                commands.trigger_targets(Connect, entity);
                            }
                            _ => {
                                commands.trigger_targets(Disconnect, entity);
                            }
                        };
                    },
                );
        });
}

pub(crate) fn update_button_text(
    query: Query<&Client>,
    mut text_query: Query<&mut Text, With<Button>>,
) {
    let Ok(client) = query.single() else {
        return;
    };
    if let Ok(mut text) = text_query.single_mut() {
        match client.state {
            ClientState::Disconnecting => {
                text.0 = "Disconnecting".to_string();
            }
            ClientState::Disconnected => {
                text.0 = "Connect".to_string();
            }
            ClientState::Connecting => {
                text.0 = "Connecting".to_string();
            }
            ClientState::Connected { .. } => {
                text.0 = "Disconnect".to_string();
            }
        }
    }
}

/// Component to identify the text displaying the client id

#[derive(Component)]
pub struct ClientIdText;

/// Listen for events to know when the client is connected, and spawn a text entity
/// to display the client id
pub(crate) fn handle_connection(
    trigger: Trigger<OnAdd, Connected>,
    query: Query<&LocalId>,
    mut commands: Commands,
) {
    let client_id = query.get(trigger.target()).unwrap().0;
    commands.spawn((
        Text(format!("Client {}", client_id)),
        TextFont::from_font_size(30.0),
        ClientIdText,
    ));
}

/// Listen for events to know when the client is disconnected, and print out the reason
/// of the disconnection
pub(crate) fn handle_disconnection(
    _trigger: Trigger<OnAdd, Disconnected>,
    mut commands: Commands,
    debug_text: Query<Entity, With<ClientIdText>>,
) {
    // TODO: add reason
    commands.trigger(UpdateStatusMessage(String::from("Disconnected")));
    for entity in debug_text.iter() {
        commands.entity(entity).despawn();
    }
}
