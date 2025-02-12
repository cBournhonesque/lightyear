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
use core::time::Duration;
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

        // despawn the existing connect button from the Renderer
        // (because we want to replace it with one with specific behaviour)
        let button_entity = app
            .world_mut()
            .query_filtered::<Entity, With<Button>>()
            .single(app.world());
        app.world_mut().despawn(button_entity);
        app.add_systems(Startup, spawn_connect_button);

        app.add_systems(Update, fetch_connect_token);
        app.add_systems(OnEnter(NetworkingState::Disconnected), on_disconnect);
    }
}

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
                info!("Using ConnectToken to connect to server.");
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

/// Remove all entities when the client disconnect
fn on_disconnect(mut commands: Commands, debug_text: Query<Entity, With<ClientIdText>>) {
    for entity in debug_text.iter() {
        commands.entity(entity).despawn();
    }
}

/// Create a button that allow you to connect/disconnect to a server
/// When pressing Connect, we will start an asynchronous request via TCP to get a ConnectToken
/// that can be used to connect
pub(crate) fn spawn_connect_button(mut commands: Commands) {
    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            align_items: AlignItems::FlexEnd,
            justify_content: JustifyContent::FlexEnd,
            flex_direction: FlexDirection::Row,
            ..default()
        })
        .with_children(|parent| {
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
                    |trigger: Trigger<Pointer<Click>>,
                     mut commands: Commands,
                     mut config: ResMut<ClientConfig>,
                     mut task_state: ResMut<ConnectTokenRequestTask>,
                     state: Res<State<NetworkingState>>| {
                        match state.get() {
                            NetworkingState::Disconnected => {
                                if let NetConfig::Netcode { auth, .. } = &config.net {
                                    if auth.has_token() {
                                        commands.connect_client();
                                        return;
                                    } else {
                                        info!("Starting task to get ConnectToken");
                                        let auth_backend_addr = task_state.auth_backend_addr;
                                        let task = IoTaskPool::get().spawn_local(Compat::new(
                                            async move {
                                                get_connect_token_from_auth_backend(
                                                    auth_backend_addr,
                                                )
                                                .await
                                            },
                                        ));
                                        task_state.task = Some(task);
                                    }
                                }
                            }
                            NetworkingState::Connecting | NetworkingState::Connected => {
                                commands.disconnect_client();
                                // reset the authentication method to None, so that we have to request a new ConnectToken
                                if let NetConfig::Netcode { auth, .. } = &mut config.net {
                                    *auth = Authentication::None;
                                }
                            }
                        };
                    },
                );
        });
}
