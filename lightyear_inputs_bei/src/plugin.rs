#[cfg(any(feature = "client", feature = "server"))]
use crate::input_message::BEIStateSequence;

use crate::setup::{ActionOfWrapper, ReplicateLike};
#[cfg(any(feature = "client", feature = "server"))]
use crate::setup::InputRegistryPlugin;
use bevy_app::FixedPreUpdate;
use bevy_app::{App, Plugin};
use bevy_ecs::component::Component;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_ecs::schedule::IntoScheduleConfigs;
#[cfg(all(feature = "client", feature = "server"))]
use bevy_ecs::schedule::common_conditions::not;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_enhanced_input::EnhancedInputSet;
use bevy_enhanced_input::context::InputContextAppExt;
use bevy_enhanced_input::prelude::ActionOf;
use bevy_reflect::TypePath;
use core::fmt::Debug;
use lightyear_inputs::config::InputConfig;
use lightyear_prediction::PredictionMode;
use lightyear_prediction::prelude::PredictionRegistrationExt;
use lightyear_replication::prelude::AppComponentExt;
use lightyear_replication::registry::replication::GetWriteFns;
use serde::Serialize;
use serde::de::DeserializeOwned;

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

impl<
    C: Component<Mutability: GetWriteFns<C>>
        + PartialEq
        + Clone
        + Debug
        + Serialize
        + DeserializeOwned
        + TypePath,
> Plugin for InputPlugin<C>
{
    fn build(&self, app: &mut App) {
        app.register_type::<ActionOf<C>>();
        if !app.is_plugin_added::<bevy_enhanced_input::EnhancedInputPlugin>() {
            app.add_plugins(bevy_enhanced_input::EnhancedInputPlugin);
        }

        app.add_input_context_to::<FixedPreUpdate, C>();
        // we register the context C entity so that it can be replicated from the server to the client
        app.register_component::<C>()
            .add_immutable_prediction(PredictionMode::Once);

        // We cannot directly replicate ActionOf<C> because it contains an entity, and we might need to do some custom mapping
        // i.e. if the Action is spawned on the predicted entity on the client, we want the ActionOf<C> entity
        // to be able to be mapped
        app.register_component::<ActionOfWrapper<C>>()
            .add_map_entities();

        // Register ReplicateLike component for input rebroadcasting
        app.register_component::<ReplicateLike<C>>()
            .add_map_entities();

        #[cfg(feature = "client")]
        {
            use crate::marker::{
                add_input_marker_from_binding, add_input_marker_from_parent, propagate_input_marker,
            };
            app.add_observer(propagate_input_marker::<C>);
            app.add_observer(add_input_marker_from_parent::<C>);
            app.add_observer(add_input_marker_from_binding::<C>);

            app.add_observer(InputRegistryPlugin::add_action_of_replicate::<C>);
            app.add_observer(InputRegistryPlugin::on_rebroadcast_action_received::<C>);

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
            app.add_observer(InputRegistryPlugin::on_action_of_replicated::<C>);

            app.add_plugins(
                lightyear_inputs::server::ServerInputPlugin::<BEIStateSequence<C>> {
                    rebroadcast_inputs: self.config.rebroadcast_inputs,
                    marker: core::marker::PhantomData,
                },
            );

            // If we are running a headless server, there is no need to run EnhancedInputSet::Update system
            #[cfg(not(feature = "client"))]
            {
                use bevy_app::PreUpdate;
                app.configure_sets(PreUpdate, EnhancedInputSet::Prepare.run_if(never));
                app.configure_sets(FixedPreUpdate, EnhancedInputSet::Update.run_if(never));
            }
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
