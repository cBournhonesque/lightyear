#[cfg(any(feature = "client", feature = "server"))]
use crate::input_message::BEIStateSequence;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_app::FixedPreUpdate;
use bevy_app::{App, Plugin};
use bevy_ecs::component::Component;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_ecs::schedule::IntoScheduleConfigs;
#[cfg(all(feature = "client", feature = "server"))]
use bevy_ecs::schedule::common_conditions::not;
use bevy_enhanced_input::context::InputContextAppExt;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_enhanced_input::EnhancedInputSet;
use lightyear_inputs::config::InputConfig;
use crate::marker::{add_input_marker_from_binding, add_input_marker_from_parent, propagate_input_marker};

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

impl<C: Component> Plugin for InputPlugin<C> {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bevy_enhanced_input::EnhancedInputPlugin>() {
            app.add_plugins(bevy_enhanced_input::EnhancedInputPlugin);
        }
        app.add_input_context::<C>();

        #[cfg(feature = "client")]
        {
            app.add_observer(propagate_input_marker::<C>);
            app.add_observer(add_input_marker_from_parent::<C>);
            app.add_observer(add_input_marker_from_binding::<C>);

            app.add_plugins(lightyear_inputs::client::ClientInputPlugin::<
                BEIStateSequence<C>,
            >::new(self.config));

            // Make sure that the BEI inputs got updated from the InputReader before buffering them
            // in the InputBuffer
            app.configure_sets(
                FixedPreUpdate,
                (
                    EnhancedInputSet::Update,
                    lightyear_inputs::client::InputSet::BufferClientInputs,
                    EnhancedInputSet::Apply,
                )
                    .chain(),
            );
        }
        #[cfg(feature = "server")]
        {
            app.add_plugins(
                lightyear_inputs::server::ServerInputPlugin::<BEIStateSequence<C>> {
                    rebroadcast_inputs: self.config.rebroadcast_inputs,
                    marker: core::marker::PhantomData,
                },
            );

            // If we are running a headless server, there is no need to run EnhancedInputSet::Update system
            #[cfg(not(feature = "client"))]
            app.configure_sets(FixedPreUpdate, EnhancedInputSet::Update.run_if(never));
            #[cfg(feature = "client")]
            {
                app.configure_sets(
                    FixedPreUpdate,
                    EnhancedInputSet::Update
                        .run_if(not(lightyear_connection::server::is_headless_server)),
                );
            }

            // Make sure that we update the ActionState using the received messages before
            // triggering BEI events
            app.configure_sets(
                FixedPreUpdate,
                lightyear_inputs::server::InputSet::UpdateActionState
                    .before(EnhancedInputSet::Apply),
            );
        }
    }
}

fn never() -> bool {
    false
}
