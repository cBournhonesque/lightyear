#[cfg(feature = "server")]
use crate::input_message::BEIBuffer;
#[cfg(any(feature = "client", feature = "server"))]
use crate::input_message::BEIStateSequence;

#[cfg(any(feature = "client", feature = "server"))]
use crate::setup::InputRegistryPlugin;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_ecs::schedule::IntoScheduleConfigs;
#[cfg(all(feature = "client", feature = "server"))]
use bevy_ecs::schedule::common_conditions::not;
#[cfg(any(feature = "client", feature = "server"))]
use bevy_enhanced_input::EnhancedInputSystems;
#[cfg(feature = "client")]
use bevy_enhanced_input::action::TriggerState;
use bevy_enhanced_input::context::InputContextAppExt;
use bevy_enhanced_input::prelude::ActionOf;
use bevy_reflect::TypePath;
use bevy_replicon::prelude::AppRuleExt;
use bevy_replicon::shared::replication::registry::receive_fns::MutWrite;
use core::fmt::Debug;
#[cfg(feature = "client")]
use lightyear_core::prelude::is_in_rollback;
#[cfg(feature = "client")]
use lightyear_inputs::client::InputSystems;
use lightyear_inputs::config::InputConfig;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Adds [`bevy_enhanced_input`] replication for an input context `C`.
///
/// This plugin registers the context component `C` for replication, sets up
/// the [`BEIStateSequence`] message protocol to send compressed action-state
/// diffs over the network, and configures the BEI scheduling so that inputs
/// are buffered and restored correctly during prediction rollbacks.
///
/// Add one `InputPlugin` per input context type in your protocol:
///
/// ```rust,ignore
/// app.add_plugins(InputPlugin::<Player>::default());
/// app.register_input_action::<Movement>();
/// ```
///
/// # Action entities
///
/// BEI uses separate "action entities" with [`ActionOf<C>`] to represent
/// individual actions. In the server-authoritative flow, spawn those action
/// entities on the server and replicate them to clients along with the context
/// entity. The owning client should add local-only [`Bindings`] once its local
/// controlled context has the replicated [`Action`] entity in its
/// [`ActionOf<C>`]/`Actions<C>` relationship.
///
/// Replicating the action entity is also what lets remote clients receive
/// rebroadcasted BEI input. Rebroadcasted [`BEIStateSequence`] messages target
/// action entities, so a remote client needs a corresponding replicated action
/// entity to resolve the target and buffer the remote player's input state.
///
/// The replicated [`Action`] component is structural: it recreates the typed BEI
/// action entity on the receiver, but does not carry runtime input state. The
/// action relationship is replicated directly through [`ActionOf<C>`].

/// Live action state is sent by [`BEIStateSequence`] input messages. The owning
/// client adds [`InputMarker`] to local action entities, buffers BEI trigger
/// state/value/time each tick, and sends those snapshots to the server. If input
/// rebroadcasting is enabled, the server forwards those input messages to other
/// clients so they can update remote action buffers for prediction.
///
/// [`Action`]: bevy_enhanced_input::prelude::Action
/// [`Bindings`]: bevy_enhanced_input::prelude::Bindings
/// [`BEIStateSequence`]: crate::input_message::BEIStateSequence
/// [`ActionOf<C>`]: bevy_enhanced_input::prelude::ActionOf
/// [`InputMarker`]: crate::marker::InputMarker
/// [`Replicate`]: lightyear_replication::prelude::Replicate
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
    C: Component<Mutability: MutWrite<C>>
        + PartialEq
        + Clone
        + Debug
        + Serialize
        + DeserializeOwned
        + TypePath,
> Plugin for InputPlugin<C>
{
    fn build(&self, app: &mut App) {
        #[cfg(feature = "server")]
        app.register_required_components::<ActionOf<C>, BEIBuffer<C>>();
        if !app.is_plugin_added::<bevy_enhanced_input::EnhancedInputPlugin>() {
            app.add_plugins(bevy_enhanced_input::EnhancedInputPlugin);
        }

        app.add_input_context_to::<FixedPreUpdate, C>();
        // we register the context C entity so that it can be replicated from the server to the client
        app.replicate::<C>();
        app.replicate_once::<ActionOf<C>>();
        #[cfg(feature = "client")]
        {
            use crate::marker::{
                add_input_marker_from_binding, add_input_marker_from_parent, propagate_input_marker,
            };
            // for rebroadcasting inputs, we insert TriggerState (which inserts the InputBuffer) when ActionOf<C> is added
            // on an entity
            app.register_required_components::<ActionOf<C>, TriggerState>();

            app.add_observer(propagate_input_marker::<C>);
            app.add_observer(add_input_marker_from_parent::<C>);
            app.add_observer(add_input_marker_from_binding::<C>);

            if self.config.rebroadcast_inputs {
                app.add_observer(InputRegistryPlugin::on_rebroadcast_action_received::<C>);
                #[cfg(feature = "server")]
                app.add_observer(InputRegistryPlugin::add_action_of_host_server_rebroadcast::<C>);
            }
            #[cfg(feature = "server")]
            {
                app.add_observer(InputRegistryPlugin::mock_non_host_owned_action::<C>);
                app.add_observer(
                    InputRegistryPlugin::mock_non_host_owned_actions_on_controlled_by::<C>,
                );
            }

            app.add_plugins(lightyear_inputs::client::ClientInputPlugin::<
                BEIStateSequence<C>,
            >::new(self.config));

            // Make sure that the BEI inputs got updated from the InputReader before buffering them
            // in the InputBuffer
            app.configure_sets(
                FixedPreUpdate,
                (
                    // do not run Update during rollback as we already know all inputs
                    EnhancedInputSystems::Update.run_if(not(is_in_rollback)),
                    InputSystems::BufferClientInputs,
                    // Apply is after BufferClientInputs so that events can re-trigger after we update the ActionState from Buffer during rollbacks
                    EnhancedInputSystems::Apply,
                )
                    .chain(),
            );
        }
        #[cfg(feature = "server")]
        {
            if self.config.rebroadcast_inputs {
                app.add_observer(InputRegistryPlugin::on_action_of_replicated::<C>);
            }

            app.add_plugins(
                lightyear_inputs::server::ServerInputPlugin::<BEIStateSequence<C>> {
                    rebroadcast_inputs: self.config.rebroadcast_inputs,
                    marker: core::marker::PhantomData,
                },
            );

            // If we are running a headless server, there is no need to run EnhancedInputSystems::Update system
            #[cfg(not(feature = "client"))]
            {
                use bevy_app::PreUpdate;
                app.configure_sets(PreUpdate, EnhancedInputSystems::Prepare.run_if(never));
                app.configure_sets(FixedPreUpdate, EnhancedInputSystems::Update.run_if(never));
            }
            #[cfg(feature = "client")]
            {
                app.configure_sets(
                    FixedPreUpdate,
                    EnhancedInputSystems::Update
                        .run_if(not(lightyear_connection::server::is_headless_server)),
                );
            }

            // Make sure that we update the ActionState using the received messages before
            // triggering BEI events
            app.configure_sets(
                FixedPreUpdate,
                lightyear_inputs::server::InputSystems::UpdateActionState
                    .before(EnhancedInputSystems::Apply),
            );
        }
    }
}

fn never() -> bool {
    false
}
