use crate::action_state::LeafwingUserAction;
use crate::input_message::LeafwingSequence;
use bevy::app::{App, FixedPreUpdate, Plugin};
use bevy::prelude::IntoScheduleConfigs;
use leafwing_input_manager::plugin::InputManagerSystem;
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
            // TODO: this means that for host-server mode InputPlugin must be added before the ProtocolPlugin!

            // we add this check so that if we only have the ServerPlugins, but the client feature is enabled,
            // we don't panic (otherwise we would because leafwing expects the bevy InputPlugin)
            // We only want the client or server leafwing plugin, not both
            if app.is_plugin_added::<bevy::input::InputPlugin>() {
                app.add_plugins(InputManagerPlugin::<A>::default());
                app.add_plugins(lightyear_inputs::client::ClientInputPlugin::<
                    LeafwingSequence<A>,
                >::new(self.config));

                // see: https://github.com/cBournhonesque/lightyear/pull/820
                app.configure_sets(
                    FixedPreUpdate,
                    lightyear_inputs::client::InputSet::RestoreInputs
                        .before(InputManagerSystem::Tick),
                );
            }
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
