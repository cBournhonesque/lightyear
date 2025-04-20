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
        app.register_required_components::<InputBuffer<ActionState<A>>, ActionState<A>>();

        // TODO: for simplicity, we currently register both client and server input plugins
        app.add_plugins(super::client::ClientInputPlugin::<A>::new(
            self.config.clone(),
        ));
        app.add_plugins(super::server::ServerInputPlugin::<A> {
            rebroadcast_inputs: self.config.rebroadcast_inputs,
            marker: core::marker::PhantomData,
        });
    }
}
