use bevy_app::App;
use bevy_ecs::prelude::*;
use bevy_utils::prelude::DebugName;
#[cfg(feature = "client")]
use {bevy_ecs::relationship::Relationship, lightyear_replication::prelude::Replicate};

use bevy_enhanced_input::prelude::*;
use lightyear_link::prelude::Server;
use lightyear_prediction::prelude::DeterministicPredicted;
use lightyear_replication::prelude::*;
use lightyear_serde::SerializationError;
use lightyear_serde::registry::SerializeFns;
use lightyear_serde::writer::Writer;
#[allow(unused_imports)]
use tracing::{debug, info};
#[cfg(feature = "server")]
use {lightyear_inputs::config::InputConfig, lightyear_replication::prelude::ReplicateLike};

pub struct InputRegistryPlugin;

impl InputRegistryPlugin {
    /// When an [`ActionOf<C>`] component is added to an entity (usually on the client),
    /// we add Replicate to it so that the action entity is also created on the server.
    #[cfg(feature = "client")]
    pub(crate) fn add_action_of_replicate<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        server: Query<(), With<Server>>,
        // we don't want to add Replicate on action entities that were already replicated
        action: Query<&ActionOf<C>, Without<Replicated>>,
        mut commands: Commands,
    ) {
        if server.single().is_ok() {
            // we're on the server, don't do anything
            return;
        }
        let entity = trigger.entity;
        if let Ok(action_of) = action.get(entity) {
            let context_entity = action_of.get();
            debug!(action_entity = ?entity, "Replicating ActionOf<{:?}> for context entity {context_entity:?} from client to server", DebugName::type_name::<C>());
            commands.entity(entity).insert((Replicate::to_server(),));
        }
    }

    /// When the server receives [`ActionOf`], optionally rebroadcast to other clients if rebroadcast_inputs is enabled
    #[cfg(feature = "server")]
    pub(crate) fn on_action_of_replicated<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        query: Query<&ActionOf<C>, With<Replicated>>,
        _: Single<(), With<Server>>,
        config: Res<InputConfig<C>>,
        mut commands: Commands,
    ) {
        let entity = trigger.entity;
        if let Ok(wrapper) = query.get(entity) {
            commands.entity(entity).remove::<Replicated>();
            debug!(?entity, context = ?DebugName::type_name::<C>(), "Server received action entity");

            // If rebroadcast_inputs is enabled, set up replication to other clients
            if config.rebroadcast_inputs {
                debug!(action_entity = ?entity, "On server, insert ReplicateLike({:?}) for action entity ActionOf<{:?}>", wrapper.get(), DebugName::type_name::<C>());

                // TODO: don't rebroadcast to the original client
                commands.entity(entity).insert((
                    ReplicateLike {
                        root: wrapper.get(),
                    },
                    // we don't want to spawn Predicted Action entities
                    PredictionTarget::manual(alloc::vec![]),
                    InterpolationTarget::manual(alloc::vec![]),
                ));
            }

            // TODO: THE PROBLEM IS THAT THE ENTITY MAPPING IS DONE PER CLIENT OF? THINK ABOUT IT
            //  HOW COME THE SERVER HAS FAILED FOR THE ACTION THAT WAS REPLICATED FROM CLIENT ?
        }
    }

    /// When the client receives a rebroadcast Action entity with [`ReplicateLike`],
    /// attach it to the correct context entity
    ///
    /// This cannot be a trigger because we need to wait until the Predicted entity is spawned
    #[cfg(feature = "client")]
    pub(crate) fn on_rebroadcast_action_received<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        query: Query<&ActionOf<C>, With<Replicated>>,
        mut commands: Commands,
    ) {
        if let Ok(action_of) = query.get(trigger.entity) {
            let entity = trigger.entity;
            debug!(
                ?entity,
                "On client, received ActionOf({:?}) for action entity ActionOf<{:?}> from input rebroadcast",
                action_of.get(),
                DebugName::type_name::<C>()
            );

            commands.entity(entity).insert((
                // We add DeterministicPredicted because lightyear_inputs::client expects the recipient
                // to be a predicted entity
                DeterministicPredicted,
                // Make sure that the actions are only updated via input messages
                bevy_enhanced_input::context::ExternallyMocked,
            ));

            // We add DeterministicPredicted because lightyear_inputs::client expects the recipient
            // to be a predicted entity
            commands.entity(entity).insert(DeterministicPredicted);
        }
    }

    // we don't care about the actual data in Action<A>, so nothing to serialize
    fn serialize_action<A: InputAction>(
        _: &Action<A>,
        _: &mut Writer,
    ) -> core::result::Result<(), SerializationError> {
        Ok(())
    }
    fn deserialize_action<A: InputAction>(
        _: &mut lightyear_serde::reader::Reader,
    ) -> core::result::Result<Action<A>, SerializationError> {
        Ok(Action::<A>::default())
    }
}

pub trait InputRegistryExt {
    /// Registers a new input action type and returns its kind.
    fn register_input_action<A: InputAction>(self) -> Self;
}

impl InputRegistryExt for &mut App {
    fn register_input_action<A: InputAction>(self) -> Self {
        // Register the Action<A> component so that it can be also added on the server
        self.register_component_custom_serde::<Action<A>>(SerializeFns::<Action<A>> {
            serialize: InputRegistryPlugin::serialize_action::<A>,
            deserialize: InputRegistryPlugin::deserialize_action::<A>,
        })
        .with_replication_config(ComponentReplicationConfig {
            replicate_once: true,
            disable: false,
            delta_compression: false,
        });

        self
    }
}
