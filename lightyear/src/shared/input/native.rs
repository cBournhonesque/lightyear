//! Plugin to register and handle user inputs.

use bevy::app::{App, Plugin};

use crate::client::config::ClientConfig;
use crate::inputs::native::InputMessage;
use crate::prelude::{ChannelDirection, UserAction};
use crate::protocol::message::{AppMessageInternalExt, MessageType};
use crate::server::config::ServerConfig;

pub struct InputPlugin<A: UserAction> {
    _marker: std::marker::PhantomData<A>,
}

impl<A: UserAction> Default for InputPlugin<A> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<A: UserAction> Plugin for InputPlugin<A> {
    fn build(&self, app: &mut App) {
        // TODO: this adds a receive_message fn that is never used! Because we have custom handling
        //  of native input message in ConnectionManager.receive()
        app.register_message_internal::<InputMessage<A>>(
            ChannelDirection::ClientToServer,
            MessageType::NativeInput,
        );
        let is_client = app.world().get_resource::<ClientConfig>().is_some();
        let is_server = app.world().get_resource::<ServerConfig>().is_some();
        assert!(is_client || is_server, "Either ClientConfig or ServerConfig must be present! Make sure that your SharedPlugin is registered after the ClientPlugins/ServerPlugins");
        if is_client {
            app.add_plugins(crate::client::input::native::InputPlugin::<A>::default());
        }
        if is_server {
            app.add_plugins(crate::server::input::native::InputPlugin::<A>::default());
        }
    }
}
