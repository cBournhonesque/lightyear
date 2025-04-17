//! Plugin to register and handle user inputs.
use crate::action_state::ActionState;
use crate::input_message::InputMessage;
use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use lightyear_inputs::config::InputConfig;
use lightyear_inputs::input_buffer::InputBuffer;
use lightyear_inputs::UserAction;
use lightyear_messages::prelude::AppMessageExt;

pub struct InputPlugin<A: UserAction> {
    pub config: InputConfig<A>,
}

impl<A: UserAction> Default for InputPlugin<A> {
    fn default() -> Self {
        Self {
            config: Default::default(),
        }
    }
}

impl<A: UserAction + MapEntities> Plugin for InputPlugin<A> {
    fn build(&self, app: &mut App) {
        // TODO: this adds a receive_message fn that is never used! Because we have custom handling
        //  of native input message in ConnectionManager.receive()
        app.add_message::<InputMessage<A>>()
            // add entity mapping for:
            // - server receiving pre-predicted entities
            // - client receiving other players' inputs
            // - input itself containing entities
            .add_map_entities();
        let is_client = app.world().get_resource::<ClientConfig>().is_some();
        let is_server = app.world().get_resource::<ServerConfig>().is_some();
        assert!(is_client || is_server, "Either ClientConfig or ServerConfig must be present! Make sure that your SharedPlugin is registered after the ClientPlugins/ServerPlugins");

        app.register_required_components::<InputBuffer<ActionState<A>>, ActionState<A>>();

        if is_client {
            app.add_plugins(super::client::InputPlugin::<A>::new(
                self.config.clone(),
            ));
        }
        if is_server {
            app.add_plugins(super::server::InputPlugin::<A> {
                rebroadcast_inputs: self.config.rebroadcast_inputs,
                marker: core::marker::PhantomData,
            });
        }
    }
}
