#[cfg(any(feature = "client", feature = "server"))]
use crate::input_message::BEIStateSequence;
use crate::marker::AddInputMarkers;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_app::FixedPreUpdate;
use bevy_app::{App, Plugin};
#[cfg(feature = "client")]
use bevy_app::{RunFixedMainLoop, RunFixedMainLoopSystem};
#[cfg(any(feature = "client", feature = "server"))]
use bevy_ecs::schedule::IntoScheduleConfigs;
#[cfg(all(feature = "client", feature = "server"))]
use bevy_ecs::schedule::common_conditions::not;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_enhanced_input::EnhancedInputSet;
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

impl<C: InputContext> Plugin for InputPlugin<C> {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bevy_enhanced_input::EnhancedInputPlugin>() {
            app.add_plugins(bevy_enhanced_input::EnhancedInputPlugin);
        }
        app.add_input_context::<C>();
        // for the component id <C>, add a type-erased function to add
        app.add_event::<AddInputMarkers<C>>();
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

            // Make sure that the BEI inputs got updated from the InputReader before buffering them
            // in the InputBuffer
            app.configure_sets(
                FixedPreUpdate,
                (
                    EnhancedInputSet::Update,
                    lightyear_inputs::client::InputSet::BufferClientInputs,
                    EnhancedInputSet::Trigger,
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
                    .before(EnhancedInputSet::Trigger),
            );
        }
    }
}

fn never() -> bool {
    false
}
