#[cfg(any(feature = "client", feature = "server"))]
use crate::input_message::BEIStateSequence;
use bevy::prelude::*;
use bevy_enhanced_input::input_context::{InputContext, InputContextAppExt};
use lightyear_inputs::config::InputConfig;

pub struct InputPlugin<C> {
    pub config: InputConfig<C>,
}

impl<C> Default for InputPlugin<C> {
    fn default() -> Self {
        Self {
            config: Default::default(),
        }
    }
}

impl<C: InputContext<Schedule = FixedPreUpdate>> Plugin for InputPlugin<C> {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bevy_enhanced_input::EnhancedInputPlugin>() {
            app.add_plugins(bevy_enhanced_input::EnhancedInputPlugin);
        }
        app.add_input_context::<C>();
        #[cfg(feature = "client")]
        {
            app.add_plugins(lightyear_inputs::client::ClientInputPlugin::<
                BEIStateSequence<C>,
            >::new(self.config));

            app.add_observer(crate::marker::create_add_input_markers_events::<C>);
            app.add_systems(
                RunFixedMainLoop,
                crate::marker::add_input_markers_system::<C>
                    .in_set(RunFixedMainLoopSystem::BeforeFixedMainLoop),
            );
        }
        #[cfg(feature = "server")]
        app.add_plugins(
            lightyear_inputs::server::ServerInputPlugin::<BEIStateSequence<C>> {
                rebroadcast_inputs: self.config.rebroadcast_inputs,
                marker: core::marker::PhantomData,
            },
        );
    }
}
