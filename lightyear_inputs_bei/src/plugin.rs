#[cfg(any(feature = "client", feature = "server"))]
use crate::input_message::BEIStateSequence;

#[cfg(any(feature = "client", feature = "server"))]
use crate::setup::InputRegistryPlugin;
use bevy_app::{PreUpdate, prelude::*};
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
use bevy_replicon::prelude::{AppRuleExt, ReplicationMode, RuleFns};
use bevy_replicon::shared::replication::registry::receive_fns::MutWrite;
use core::fmt::Debug;
#[cfg(feature = "client")]
use lightyear_core::prelude::is_in_rollback;
#[cfg(feature = "client")]
use lightyear_inputs::client::InputSystems;
use lightyear_inputs::config::InputConfig;
#[cfg(any(feature = "client", feature = "server"))]
use lightyear_messages::plugin::MessageSystems;
#[cfg(any(feature = "client", feature = "server"))]
use lightyear_replication::ReplicationSystems;
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
/// individual actions. These entities need to exist on both client and server.
/// The recommended approach is to use [`PreSpawned`] so both sides spawn them
/// independently and match via a deterministic hash — this avoids the need
/// for client-to-server entity replication.
///
/// [`BEIStateSequence`]: crate::input_message::BEIStateSequence
/// [`ActionOf<C>`]: bevy_enhanced_input::prelude::ActionOf
/// [`PreSpawned`]: lightyear_replication::prelude::PreSpawned
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
        app.register_type::<ActionOf<C>>();
        if !app.is_plugin_added::<bevy_enhanced_input::EnhancedInputPlugin>() {
            app.add_plugins(bevy_enhanced_input::EnhancedInputPlugin);
        }

        app.add_input_context_to::<FixedPreUpdate, C>();
        // we register the context C entity so that it can be replicated from the server to the client
        app.replicate::<C>();

        // We mirror ActionOf<C> into a separate component that stores the authoritative
        // remote entity. That avoids depending on sender-side entity mapping in replicon's
        // SerializeCtx.
        app.replicate_with((
            RuleFns::new(
                crate::setup::serialize_network_action_of::<C>,
                crate::setup::deserialize_network_action_of::<C>,
            ),
            ReplicationMode::default(),
        ));
        app.add_observer(InputRegistryPlugin::mirror_action_of_for_replication::<C>);
        app.add_observer(InputRegistryPlugin::insert_action_of_from_network::<C>);
        app.add_systems(
            PreUpdate,
            (
                InputRegistryPlugin::resolve_pending_network_action_of::<C>,
                InputRegistryPlugin::resolve_pending_action_of::<C>,
            )
                .chain()
                .after(ReplicationSystems::Receive)
                .before(MessageSystems::Receive),
        );

        #[cfg(feature = "client")]
        {
            use crate::marker::{
                add_input_marker_from_authority, add_input_marker_from_binding,
                add_input_marker_from_confirmed_controlled_action,
                add_input_marker_from_network_action, add_input_marker_from_parent,
                propagate_input_marker,
            };
            // for rebroadcasting inputs, we insert TriggerState (which inserts the InputBuffer) when ActionOf<C> is added
            // on an entity
            app.register_required_components::<ActionOf<C>, TriggerState>();

            app.add_observer(propagate_input_marker::<C>);
            app.add_observer(add_input_marker_from_parent::<C>);
            app.add_observer(add_input_marker_from_binding::<C>);
            app.add_observer(add_input_marker_from_authority::<C>);
            app.add_observer(add_input_marker_from_network_action::<C>);
            app.add_observer(add_input_marker_from_confirmed_controlled_action::<C>);

            if self.config.rebroadcast_inputs {
                app.add_observer(InputRegistryPlugin::on_rebroadcast_action_received::<C>);
                #[cfg(feature = "server")]
                app.add_observer(InputRegistryPlugin::add_action_of_host_server_rebroadcast::<C>);
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
