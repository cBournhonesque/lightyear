use bevy::prelude::*;
use lightyear::connection::server::Started;
use lightyear::prelude::server::{Server, Start, Stop, Stopped};

pub struct ExampleServerRendererPlugin {
    /// The name of the example, which must also match the edgegap application name.
    pub name: String,
}

impl ExampleServerRendererPlugin {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

#[derive(Resource)]
struct GameName(String);

impl Plugin for ExampleServerRendererPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(GameName(self.name.clone()));
        app.insert_resource(ClearColor::default());
        // TODO common shortcuts for enabling the egui world inspector etc.
        // TODO handle bevygap ui things.
        app.add_systems(Startup, set_window_title);
        app.add_systems(Startup, spawn_server_text);

        spawn_start_button(app);
        app.add_systems(Update, update_button_text);
    }
}

fn set_window_title(mut window: Query<&mut Window>, game_name: Res<GameName>) {
    let mut window = window.single_mut().unwrap();
    window.title = format!("Lightyear Example: {}", game_name.0);
}

/// Spawns a text element that displays "Server"
fn spawn_server_text(mut commands: Commands) {
    commands.spawn((
        Text("Server".to_string()),
        TextFont::from_font_size(30.0),
        TextColor(Color::WHITE.with_alpha(0.5)),
        Node {
            align_self: AlignSelf::End,
            ..default()
        },
    ));
}

/// Create a button that allow you to start/stop the server
pub(crate) fn spawn_start_button(app: &mut App) {
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
                Node {
                    padding: UiRect::all(Val::Px(10.0)),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
            ));
            parent
                .spawn((
                    Text("Start".to_string()),
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
                     query: Single<(Entity, Has<Started>, Has<Stopped>), With<Server>>| {
                        let (entity, started, stopped) = query.into_inner();
                        if started {
                            info!("Stopping server");
                            commands.trigger_targets(Stop, entity)
                        }
                        if stopped {
                            info!("Starting server");
                            commands.trigger_targets(Start, entity)
                        }
                    },
                );
        });
}

pub(crate) fn update_button_text(
    server: Single<(Has<Started>, Has<Stopped>), With<Server>>,
    mut text: Single<&mut Text, With<Button>>,
) {
    let (started, stopped) = server.into_inner();
    if started {
        text.0 = "Stop".to_string();
    }
    if stopped {
        text.0 = "Start".to_string();
    }
}
