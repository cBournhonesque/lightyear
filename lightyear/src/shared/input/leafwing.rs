//! Plugin to register and handle user inputs.

use crate::client::config::ClientConfig;
use crate::inputs::leafwing::input_buffer::InputBuffer;
use crate::prelude::{ChannelDirection, InputMessage, LeafwingUserAction};
use crate::protocol::message::registry::AppMessageInternalExt;
use crate::server::config::ServerConfig;
use crate::shared::input::InputConfig;
use bevy::app::{App, Plugin};
use leafwing_input_manager::prelude::ActionState;

pub struct LeafwingInputPlugin<A> {
    pub config: InputConfig<A>,
}

impl<A> Default for LeafwingInputPlugin<A> {
    fn default() -> Self {
        Self {
            config: Default::default(),
        }
    }
}

impl<A: LeafwingUserAction> Plugin for LeafwingInputPlugin<A> {
    fn build(&self, app: &mut App) {
        let is_client = app.world().get_resource::<ClientConfig>().is_some();
        let is_server = app.world().get_resource::<ServerConfig>().is_some();

        assert!(
            is_client || is_server,
            "LeafwingInputPlugin must be added after the Client/Server plugins have been added"
        );

        app.register_required_components::<ActionState<A>, InputBuffer<A>>();
        app.register_required_components::<InputBuffer<A>, ActionState<A>>();
        // app.register_required_components::<InputMap<A>, ActionState<A>>();
        if is_client {
            app.add_plugins(
                crate::client::input::leafwing::LeafwingInputPlugin::<A>::new(self.config),
            );
        }
        if is_server {
            app.add_plugins(crate::server::input::leafwing::LeafwingInputPlugin::<A> {
                rebroadcast_inputs: self.config.rebroadcast_inputs,
                marker: core::marker::PhantomData,
            });
        }
    }

    // we build this in `finish` to be sure that the MessageRegistry, ClientConfig, ServerConfig exists
    fn finish(&self, app: &mut App) {
        // TODO: this creates a receive_message fn for InputMessage that is never use as we have
        //  custom handling of LeafwingInputMessage
        // leafwing messages have special handling so we register them as LeafwingInput
        // we still use `add_message_internal` because we want to emit events contain the message
        // so the user can inspect them and re-broadcast them to other players
        app.register_message_internal::<InputMessage<A>>(ChannelDirection::Bidirectional)
            // add entity mapping for:
            // - server receiving pre-predicted entities
            // - client receiving other players' inputs
            .add_map_entities();

        // NOTE: no need to replicate ActionState because we will insert the ActionState + InputBuffer on server
        //   as soon as we receive an InputMessage!

        // // Note: this is necessary because
        // // - so that the server entity has an ActionState on the server when the ActionState is added on the client
        // //   (we only replicate it once when ActionState is first added)
        // // - we don't need to replicate from server->client because we will add ActionState on any entity
        // //   where the client adds an InputMap
        // app.register_component::<ActionState<A>>(ChannelDirection::ClientToServer);
    }
}
