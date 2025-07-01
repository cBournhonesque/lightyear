use crate::action_state::LeafwingUserAction;
#[cfg(feature = "client")]
use bevy_app::FixedPreUpdate;
use bevy_app::{App, Plugin};
#[cfg(feature = "client")]
use bevy_ecs::schedule::IntoScheduleConfigs;
use leafwing_input_manager::action_state::ActionState;
use lightyear_inputs::config::InputConfig;
use lightyear_inputs::input_buffer::InputBuffer;

#[cfg(any(feature = "client", feature = "server"))]
use crate::input_message::LeafwingSequence;
#[cfg(any(feature = "client", feature = "server"))]
use leafwing_input_manager::plugin::InputManagerPlugin;

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
        app.register_type::<InputBuffer<ActionState<A>>>();
        app.register_type::<ActionState<A>>();

        #[cfg(feature = "client")]
        {
            use leafwing_input_manager::plugin::InputManagerSystem;
            // TODO: this means that for host-server mode InputPlugin must be added before the ProtocolPlugin!

            // we add this check so that if we only have the ServerPlugins, but the client feature is enabled,
            // we don't panic (otherwise we would because leafwing expects the bevy InputPlugin)
            // We only want the client or server leafwing plugin, not both
            if app.is_plugin_added::<bevy_input::InputPlugin>() {
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
