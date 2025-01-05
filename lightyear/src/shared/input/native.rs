//! Plugin to register and handle user inputs.

use bevy::app::{App, Plugin};

use crate::client::config::ClientConfig;
use crate::inputs::native::InputMessage;
use crate::prelude::{MessageRegistry, UserAction};
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
    fn build(&self, app: &mut App) {}

    fn finish(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<MessageRegistry>()
            .add_message::<InputMessage<A>>();
        // TODO: add MessageType!
        let is_client = app.world().get_resource::<ClientConfig>().is_some();
        let is_server = app.world().get_resource::<ServerConfig>().is_some();
        if is_client {
            app.add_plugins(crate::client::input::native::InputPlugin::<A>::default());
        }
        if is_server {
            app.add_plugins(crate::server::input::native::InputPlugin::<A>::default());
        }
    }
}
