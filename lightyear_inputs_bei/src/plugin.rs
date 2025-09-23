#[cfg(any(feature = "client", feature = "server"))]
use crate::input_message::BEIStateSequence;

use crate::setup::ActionOfWrapper;
#[cfg(any(feature = "client", feature = "server"))]
use crate::setup::InputRegistryPlugin;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_ecs::schedule::IntoScheduleConfigs;
#[cfg(all(feature = "client", feature = "server"))]
use bevy_ecs::schedule::common_conditions::not;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_enhanced_input::EnhancedInputSet;
use bevy_enhanced_input::action::ActionState;
use bevy_enhanced_input::context::InputContextAppExt;
use bevy_enhanced_input::prelude::ActionOf;
use bevy_reflect::TypePath;
use core::fmt::Debug;
use lightyear_core::prelude::is_in_rollback;
#[cfg(feature = "client")]
use lightyear_inputs::client::InputSet;
use lightyear_inputs::config::InputConfig;
use lightyear_prediction::plugin::PredictionSet;
use lightyear_replication::prelude::AppComponentExt;
use lightyear_replication::registry::replication::GetWriteFns;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Add BEI Input replication to your app.
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

impl<C> InputPlugin<C> {
    pub fn new(config: InputConfig<C>) -> Self {
        Self { config }
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
        app.register_component::<C>();

        // We cannot directly replicate ActionOf<C> because it contains an entity, and we might need to do some custom mapping
        // i.e. if the Action is spawned on the predicted entity on the client, we want the ActionOf<C> entity
        // to be able to be mapped
        app.register_component::<ActionOfWrapper<C>>()
            .add_map_entities();

        #[cfg(feature = "client")]
        {
            use crate::marker::{
                add_input_marker_from_binding, add_input_marker_from_parent, propagate_input_marker,
            };
            // for rebroadcasting inputs, we insert ActionState (which inserts the InputBuffer) when ActionOf<C> is added
            // on an entity
            app.register_required_components::<ActionOf<C>, ActionState>();

            app.add_observer(propagate_input_marker::<C>);
            app.add_observer(add_input_marker_from_parent::<C>);
            app.add_observer(add_input_marker_from_binding::<C>);

            if self.config.rebroadcast_inputs {
                #[cfg(feature = "client")]
                app.add_systems(
                    PreUpdate,
                    InputRegistryPlugin::on_rebroadcast_action_received::<C>
                        // we need to wait for the predicted Context entity to be spawned first
                        .after(PredictionSet::Sync),
                );
            }

            app.add_observer(InputRegistryPlugin::add_action_of_replicate::<C>);

            app.add_plugins(lightyear_inputs::client::ClientInputPlugin::<
                BEIStateSequence<C>,
            >::new(self.config));

            // Make sure that the BEI inputs got updated from the InputReader before buffering them
            // in the InputBuffer
            app.configure_sets(
                FixedPreUpdate,
                (
                    // do not run Update during rollback as we already know all inputs
                    EnhancedInputSet::Update.run_if(not(is_in_rollback)),
                    InputSet::BufferClientInputs,
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
