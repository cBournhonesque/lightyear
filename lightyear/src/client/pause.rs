//! Handles pausing the client connection on the web when the tab is in the background.
//!
//! When a tab is put in the background the connection is throttled to save battery in the browser.
//! This means that the scheduler stops running and the bevy app pauses.
//! This normally means that the connection will be disconnected because we stop sending and receiving
//! keep-alive packets.
//!
//! What we can do is send a message to the server to notify them to pause the connection.

use crate::client::connection::ConnectionManager;
use crate::client::networking::is_connected;
use crate::connection::client::NetClient;
use crate::prelude::client::{ClientConfig, ClientConnection, NetConfig};
use crate::prelude::{DefaultUnorderedUnreliableChannel, SharedConfig};
use crate::server::pause::PauseMessage;
use bevy::prelude::*;
use bevy::utils::SystemTime;
use bevy::window::WindowOccluded;
use tracing::error;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::JsCast;

pub struct PausePlugin;

impl Plugin for PausePlugin {
    fn build(&self, app: &mut App) {
        // app.add_systems(
        //     PreUpdate,
        //     pause_tab.run_if(is_connected.and_then(not(SharedConfig::is_host_server_condition))),
        // );
        add_pause_tab();
        app.add_systems(Last, debug);
    }
}

fn debug(mut events: EventReader<WindowOccluded>) {
    for event in events.read() {
        let window = event.window;
        let time = SystemTime::now();
        if event.occluded {
            error!(?time, ?window, "Debugging pause plugin");
        } else {
            error!(?time, ?window, "Debugging unpause plugin");
        }
    }
}

fn add_pause_tab() {
    let window = web_sys::window().unwrap();
    {
        let closure = Closure::<dyn FnMut(_)>::new(move |event: web_sys::PageTransitionEvent| {
            error!("Page is hidden");
        });
        window.add_event_listener_with_callback("pagehide", closure.as_ref().unchecked_ref());
        closure.forget();
    }
    {
        let closure = Closure::<dyn FnMut(_)>::new(move |event: web_sys::PageTransitionEvent| {
            error!("Page is shown");
        });
        window.add_event_listener_with_callback("pageshow", closure.as_ref().unchecked_ref());
        closure.forget();
    }
}

/// Detect that a tab has been put in the background on the web; which means that the scheduler
/// is going to be throttled. Send a message to the server to ask the connection to be paused.
fn unpause_tab(
    config: Res<ClientConfig>,
    mut events: EventReader<WindowOccluded>,
    mut manager: ResMut<ConnectionManager>,
    mut connection: ResMut<ClientConnection>,
) {
    for event in events.read() {
        let window = event.window;
        let time = SystemTime::now();
        if event.occluded {
            // Send a message to server to pause the connection
            // Enter networking state Paused?
            error!(?time, ?window, "Tab is occluded, pausing connection");
            // TODO: maybe send it multiple times for packet loss
            manager
                .send_message::<DefaultUnorderedUnreliableChannel, _>(&PauseMessage {
                    paused: true,
                })
                .unwrap();
            if let NetConfig::Netcode { config, .. } = &config.net {
                error!(
                    "Setting timeout to {} seconds",
                    config.paused_client_timeout_secs
                );
                connection
                    .set_timeout(config.paused_client_timeout_secs)
                    .unwrap();
            }
        } else {
            error!(?time, ?window, "Tab is not occluded, unpausing connection");
            manager
                .send_message::<DefaultUnorderedUnreliableChannel, _>(&PauseMessage {
                    paused: false,
                })
                .unwrap();
            if let NetConfig::Netcode { config, .. } = &config.net {
                error!("Setting timeout to {} seconds", config.client_timeout_secs);
                connection.set_timeout(config.client_timeout_secs).unwrap();
            }
        }
    }
}
