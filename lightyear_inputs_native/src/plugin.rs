//! Plugin to register and handle user inputs.
use crate::input_message::NativeStateSequence;
use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use core::fmt::Debug;
use lightyear_inputs::config::InputConfig;
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
        // TODO: for simplicity, we currently register both client and server input plugins
        #[cfg(feature="client")]
        {
            use lightyear_inputs::client::ClientInputPlugin;
            app.add_plugins(ClientInputPlugin::<NativeStateSequence<A>>::new(
                self.config.clone(),
            ));
        }

        #[cfg(feature="server")]
        {
            use lightyear_inputs::server::ServerInputPlugin;
            app.add_plugins(ServerInputPlugin::<NativeStateSequence<A>> {
                rebroadcast_inputs: self.config.rebroadcast_inputs,
                marker: core::marker::PhantomData,
            });
        }
    }
}
