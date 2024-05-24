//! Plugin to register and handle user inputs.

use bevy::app::{App, Plugin};
use leafwing_input_manager::prelude::ActionState;

use crate::client::config::ClientConfig;
use crate::client::input::leafwing::LeafwingInputConfig;
use crate::inputs::leafwing::InputMessage;
use crate::prelude::client::ComponentSyncMode;
use crate::prelude::{
    AppComponentExt, AppMessageExt, AppSerializeExt, ChannelDirection, LeafwingUserAction,
    MessageRegistry,
};
use crate::protocol::message::AppMessageInternalExt;
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
        // leafwing messages have special handling so we register them as LeafwingInput
        // we still use `add_message_internal` because we want to emit events contain the message
        // so the user can inspect them and re-broadcast them to other players
        app.add_message_internal::<InputMessage<A>>(
            ChannelDirection::Bidirectional,
            MessageType::LeafwingInput,
        )
        // add entity mapping for:
        // - server receiving pre-predicted entities
        // - client receiving other players' inputs
        .add_map_entities();
        // TODO: how can we avoid this?
        // We still need to replicate the ActionState to the client
        app.register_component::<ActionState<A>>(ChannelDirection::Bidirectional)
            .add_prediction(ComponentSyncMode::Simple);
        let is_client = app.world.get_resource::<ClientConfig>().is_some();
        let is_server = app.world.get_resource::<ServerConfig>().is_some();
        if is_client {
            app.add_plugins(
                crate::client::input::leafwing::LeafwingInputPlugin::<A>::new(self.config),
            );
        }
        if is_server {
            app.add_plugins(crate::server::input::leafwing::LeafwingInputPlugin::<A>::default());
        }
    }
}
