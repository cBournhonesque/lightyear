//! Plugin to register and handle user inputs.

use crate::action_state::ActionState;
use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use core::fmt::Debug;
use lightyear_inputs::client::ClientInputPlugin;
use lightyear_inputs::config::InputConfig;
use lightyear_inputs::input_buffer::InputBuffer;
use lightyear_inputs::server::ServerInputPlugin;
use serde::de::DeserializeOwned;
use serde::Serialize;

pub struct InputPlugin<A> {
    pub config: InputConfig<A>,
}

impl<A> Default for InputPlugin<A> {
    fn default() -> Self {
        Self {
            config: Default::default(),
        }
    }
}

impl<A: Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + Debug + 'static + MapEntities> Plugin for InputPlugin<A> {
    fn build(&self, app: &mut App) {
        app.register_required_components::<InputBuffer<ActionState<A>>, ActionState<A>>();

        // TODO: for simplicity, we currently register both client and server input plugins
        #[cfg(feature="client")]
        app.add_plugins(ClientInputPlugin::<A>::new(
            self.config.clone(),
        ));
        #[cfg(feature="server")]
        app.add_plugins(ServerInputPlugin::<A> {
            rebroadcast_inputs: self.config.rebroadcast_inputs,
            marker: core::marker::PhantomData,
        });
    }
}
