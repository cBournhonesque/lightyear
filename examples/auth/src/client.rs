//! The client plugin.
//! The client will be responsible for:
//! - connecting to the server at Startup
//! - sending inputs to the server
//! - applying inputs to the locally predicted player (for prediction to work, inputs have to be applied to both the
//! predicted entity and the server entity)
use async_compat::Compat;
use std::io::Read;
use std::net::SocketAddr;
use std::str::FromStr;

use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{block_on, IoTaskPool, Task};
use bevy::time::common_conditions::on_timer;
use bevy::utils::Duration;
use bevy_mod_picking::picking_core::Pickable;
use bevy_mod_picking::prelude::{Click, On, Pointer};
use lightyear::connection::netcode::CONNECT_TOKEN_BYTES;
use tokio::io::AsyncReadExt;

use lightyear::prelude::client::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;

pub struct ExampleClientPlugin {
    pub auth_backend_address: SocketAddr,
}

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ConnectTokenRequestTask {
            auth_backend_addr: self.auth_backend_address,
            task: None,
        });
        app.add_systems(Startup, spawn_connect_button);
        app.add_systems(PreUpdate, handle_connection.after(MainSet::Receive));
        app.add_systems(Update, button_system);
        app.add_systems(Update, fetch_connect_token);
        app.add_systems(OnEnter(NetworkingState::Disconnected), on_disconnect);
    }
}

///

/// Holds a handle to an io task that is requesting a `ConnectToken` from the backend
#[derive(Resource)]
struct ConnectTokenRequestTask {
    auth_backend_addr: SocketAddr,
    task: Option<Task<ConnectToken>>,
}

/// If we have a io task that is waiting for a `ConnectToken`, we poll the task until completion,
/// then we retrieve the token and connect to the game server
fn fetch_connect_token(
    mut connect_token_request: ResMut<ConnectTokenRequestTask>,
    mut client_config: ResMut<ClientConfig>,
    mut commands: Commands,
) {
    if let Some(task) = &mut connect_token_request.task {
        if let Some(connect_token) = block_on(future::poll_once(task)) {
            // if we have received the connect token, update the `ClientConfig` to use it to connect
            // to the game server
            if let NetConfig::Netcode { auth, .. } = &mut client_config.net {
                *auth = Authentication::Token(connect_token);
            }
            commands.connect_client();
            connect_token_request.task = None;
        }
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

/// Get a ConnectToken via a TCP connection to the authentication server
async fn get_connect_token_from_auth_backend(auth_backend_address: SocketAddr) -> ConnectToken {
    let stream = tokio::net::TcpStream::connect(auth_backend_address)
        .await
        .expect(
            format!(
                "Failed to connect to authentication server on {:?}",
                auth_backend_address
            )
            .as_str(),
        );
    // wait for the socket to be readable
    stream.readable().await.unwrap();
    let mut buffer = [0u8; CONNECT_TOKEN_BYTES];
    match stream.try_read(&mut buffer) {
        Ok(n) if n == CONNECT_TOKEN_BYTES => {
            trace!(
                "Received token bytes: {:?}. Token len: {:?}",
                buffer,
                buffer.len()
            );
            ConnectToken::try_from_bytes(&buffer)
                .expect("Failed to parse token from authentication server")
        }
        _ => {
            panic!("Failed to read token from authentication server")
        }
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
                        background_color: Color::rgb(0.15, 0.15, 0.15).into(),
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
                                color: Color::rgb(0.9, 0.9, 0.9),
                                ..default()
                            },
                        ),
                        Pickable::IGNORE,
                    ));
                });
        });
}

/// Remove all entities when the client disconnect
fn on_disconnect(mut commands: Commands, debug_text: Query<Entity, With<ClientIdText>>) {
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
                    *on_click = On::<Pointer<Click>>::run(
                        |mut commands: Commands,
                         config: Res<ClientConfig>,
                         mut task_state: ResMut<ConnectTokenRequestTask>| {
                            if let NetConfig::Netcode { auth, .. } = &config.net {
                                // if we have a connect token, try to connect to the game server
                                if auth.has_token() {
                                    commands.connect_client();
                                    return;
                                } else {
                                    let auth_backend_addr = task_state.auth_backend_addr;
                                    let task =
                                        IoTaskPool::get().spawn_local(Compat::new(async move {
                                            get_connect_token_from_auth_backend(auth_backend_addr)
                                                .await
                                        }));
                                    task_state.task = Some(task);
                                }
                            }
                        },
                    );
                }
                NetworkingState::Connecting => {
                    text.sections[0].value = "Connecting".to_string();
                    *on_click = On::<Pointer<Click>>::run(|| {});
                }
                NetworkingState::Connected => {
                    text.sections[0].value = "Disconnect".to_string();
                    *on_click = On::<Pointer<Click>>::run(
                        |mut commands: Commands, mut config: ResMut<ClientConfig>| {
                            commands.disconnect_client();
                            // reset the authentication method to None, so that we have to request a new ConnectToken
                            if let NetConfig::Netcode { auth, .. } = &mut config.net {
                                *auth = Authentication::None;
                            }
                        },
                    );
                }
            };
        }
    }
}
