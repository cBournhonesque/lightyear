//! Plugin to register and handle user inputs.
use crate::action_state::ActionState;
#[cfg(any(feature = "client", feature = "server"))]
use crate::input_message::NativeStateSequence;
use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::FromReflect;
use bevy::reflect::Reflectable;
use core::fmt::Debug;
use lightyear_inputs::config::InputConfig;
use lightyear_inputs::input_buffer::InputBuffer;
use serde::Serialize;
use serde::de::DeserializeOwned;

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

impl<
    A: Serialize
        + DeserializeOwned
        + Clone
        + PartialEq
        + Send
        + Sync
        + Debug
        + 'static
        + MapEntities
        + Reflectable
        + FromReflect,
> Plugin for InputPlugin<A>
{
    fn build(&self, app: &mut App) {
        app.register_type::<InputBuffer<ActionState<A>>>();
        app.register_type::<ActionState<A>>();

        // TODO: for simplicity, we currently register both client and server input plugins if both features are enabled
        #[cfg(feature = "client")]
        {
            use lightyear_inputs::client::ClientInputPlugin;
            app.add_plugins(ClientInputPlugin::<NativeStateSequence<A>>::new(
                self.config,
            ));
        }

        #[cfg(feature = "server")]
        {
            use lightyear_inputs::server::ServerInputPlugin;
            app.add_plugins(ServerInputPlugin::<NativeStateSequence<A>> {
                rebroadcast_inputs: self.config.rebroadcast_inputs,
                marker: core::marker::PhantomData,
            });
        }
    }
}
