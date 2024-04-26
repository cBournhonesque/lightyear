//! Plugin to register and handle user inputs.

use crate::_reexport::ClientMarker;
use crate::client::config::ClientConfig;
use crate::inputs::leafwing::InputMessage;
use crate::prelude::{LeafwingUserAction, MessageRegistry};
use crate::protocol::message::MessageType;
use crate::server::config::ServerConfig;
use bevy::app::{App, Plugin};
use tracing::error;

pub struct LeafwingInputPlugin<A> {
    _marker: std::marker::PhantomData<A>,
}

impl<A> Default for LeafwingInputPlugin<A> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<A: LeafwingUserAction> Plugin for LeafwingInputPlugin<A> {
    fn build(&self, app: &mut App) {}

    fn finish(&self, app: &mut App) {
        app.world
            .resource_mut::<MessageRegistry>()
            .add_message::<InputMessage<A>>(MessageType::LeafwingInput);
        let is_client = app.world.get_resource::<ClientConfig>().is_some();
        let is_server = app.world.get_resource::<ServerConfig>().is_some();
        if is_client {
            app.add_plugins(crate::client::input::InputPlugin::<A>::default());
        }
        if is_server {
            app.add_plugins(crate::server::input::InputPlugin::<A>::default());
        }
    }
}
