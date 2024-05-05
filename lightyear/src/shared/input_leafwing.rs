//! Plugin to register and handle user inputs.

use bevy::app::{App, Plugin};
use leafwing_input_manager::prelude::ActionState;

use crate::client::config::ClientConfig;
use crate::client::input_leafwing::LeafwingInputConfig;
use crate::inputs::leafwing::InputMessage;
use crate::prelude::{
    AppComponentExt, AppMessageExt, AppSerializeExt, ChannelDirection, LeafwingUserAction,
    MessageRegistry,
};
use crate::protocol::message::MessageType;
use crate::server::config::ServerConfig;

pub struct LeafwingInputPlugin<A> {
    pub config: LeafwingInputConfig<A>,
}

impl<A> Default for LeafwingInputPlugin<A> {
    fn default() -> Self {
        Self {
            config: Default::default(),
        }
    }
}

impl<A: LeafwingUserAction> Plugin for LeafwingInputPlugin<A> {
    fn build(&self, app: &mut App) {}

    // we build this in `finish` to be sure that the MessageRegistry, ClientConfig, ServerConfig exists
    fn finish(&self, app: &mut App) {
        app.world
            .resource_mut::<MessageRegistry>()
            .add_message::<InputMessage<A>>(MessageType::LeafwingInput);
        app.add_map_entities::<InputMessage<A>>();
        app.register_component::<ActionState<A>>(ChannelDirection::Bidirectional);
        let is_client = app.world.get_resource::<ClientConfig>().is_some();
        let is_server = app.world.get_resource::<ServerConfig>().is_some();
        if is_client {
            app.add_plugins(
                crate::client::input_leafwing::LeafwingInputPlugin::<A>::new(self.config),
            );
        }
        if is_server {
            app.add_plugins(crate::server::input_leafwing::LeafwingInputPlugin::<A>::default());
        }
    }
}
