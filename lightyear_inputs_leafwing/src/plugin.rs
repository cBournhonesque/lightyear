use crate::action_state::LeafwingUserAction;
use crate::input_message::LeafwingSequence;
use bevy::app::{App, Plugin};
use leafwing_input_manager::prelude::InputManagerPlugin;
use lightyear_inputs::config::InputConfig;

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

impl<A: LeafwingUserAction> Plugin for InputPlugin<A> {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "client")]
        {
            app.add_plugins(InputManagerPlugin::<A>::default());
            app.add_plugins(lightyear_inputs::client::ClientInputPlugin::<
                LeafwingSequence<A>,
            >::new(self.config));
        }
        #[cfg(feature = "server")]
        app.add_plugins(
            lightyear_inputs::server::ServerInputPlugin::<LeafwingSequence<A>> {
                rebroadcast_inputs: self.config.rebroadcast_inputs,
                marker: core::marker::PhantomData,
            },
        );
    }

    fn finish(&self, app: &mut App) {
        #[cfg(feature = "server")]
        if !app.is_plugin_added::<InputManagerPlugin<A>>() {
            app.add_plugins(InputManagerPlugin::<A>::server());
        }
    }
}
