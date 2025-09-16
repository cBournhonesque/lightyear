use bevy_app::App;
use bevy_ecs::entity::MapEntities;
use bevy_ecs::prelude::*;
#[cfg(feature = "client")]
use {
    bevy_ecs::relationship::Relationship, lightyear_core::prediction::Predicted,
    lightyear_replication::prelude::Replicate,
};

use bevy_enhanced_input::prelude::*;
use bevy_reflect::Reflect;
use lightyear_link::prelude::Server;
use lightyear_prediction::prelude::DeterministicPredicted;
use lightyear_replication::components::Confirmed;
use lightyear_replication::prelude::*;
use lightyear_serde::SerializationError;
use lightyear_serde::registry::SerializeFns;
use lightyear_serde::writer::Writer;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use tracing::{debug, info};
#[cfg(feature = "server")]
use {lightyear_inputs::config::InputConfig, lightyear_replication::prelude::ReplicateLike};

/// Wrapper around [`ActionOf<C>`] that is needed for replication with custom entity mapping
#[derive(Component, Serialize, Deserialize, Reflect)]
pub struct ActionOfWrapper<C> {
    context: Entity,
    #[reflect(ignore)]
    marker: core::marker::PhantomData<C>,
}

impl<C> PartialEq for ActionOfWrapper<C> {
    fn eq(&self, other: &Self) -> bool {
        self.context == other.context
    }
}

impl<C> Clone for ActionOfWrapper<C> {
    fn clone(&self) -> Self {
        Self {
            context: self.context,
            marker: core::marker::PhantomData,
        }
    }
}

impl<C> ActionOfWrapper<C> {
    pub fn new(context: Entity) -> Self {
        Self {
            context,
            marker: core::marker::PhantomData,
        }
    }
}

impl<C> MapEntities for ActionOfWrapper<C> {
    fn map_entities<E: EntityMapper>(&mut self, entity_mapper: &mut E) {
        self.context = entity_mapper.get_mapped(self.context);
    }
}

pub struct InputRegistryPlugin;

impl InputRegistryPlugin {
    /// When an [`ActionOf<C>`] component is added to an entity (usually on the client),
    /// we add Replicate to it so that the action entity is also created on the server.
    #[cfg(feature = "client")]
    pub(crate) fn add_action_of_replicate<C: Component>(
        trigger: Trigger<OnAdd, ActionOf<C>>,
        server: Query<(), With<Server>>,
        // we don't want to add Replicate on action entities that were already replicated
        action: Query<&ActionOf<C>, Without<Replicated>>,
        query: Query<Option<&Predicted>>,
        mut commands: Commands,
    ) {
        if server.single().is_ok() {
            // we're on the server, don't do anything
            return;
        }
        let entity = trigger.target();
        if let Ok(action_of) = action.get(entity)
            && let Ok(predicted) = query.get(action_of.get())
        {
            // we replicate using the confirmed entity so that the server can map it to the server entity
            let context_entity = predicted.map_or(action_of.get(), |p| p.confirmed_entity.unwrap());
            debug!(action_entity = ?entity, "Replicating ActionOf<{:?}> for confirmed entity {context_entity:?} from client to server", core::any::type_name::<C>());
            commands.entity(entity).insert((
                ActionOfWrapper::<C>::new(context_entity),
                Replicate::to_server(),
            ));
            if predicted.is_some() {
                // we can remove the context from the Confirmed entity since the Actions are on the Predicted entity
                commands.entity(context_entity).remove_with_requires::<C>();
            }
        }
    }

    /// When the server receives [`ActionOfWrapper`], insert the appropriate [`ActionOf`]
    /// and optionally rebroadcast to other clients if rebroadcast_inputs is enabled
    #[cfg(feature = "server")]
    pub(crate) fn on_action_of_replicated<C: Component>(
        trigger: Trigger<OnAdd, ActionOfWrapper<C>>,
        query: Query<&ActionOfWrapper<C>, With<Replicated>>,
        is_server: Single<(), With<Server>>,
        config: Res<InputConfig<C>>,
        mut commands: Commands,
    ) {
        let entity = trigger.target();
        if let Ok(wrapper) = query.get(entity) {
            commands
                .entity(entity)
                .insert(ActionOf::<C>::new(wrapper.context))
                .remove::<Replicated>();
            debug!(?entity, context = ?core::any::type_name::<C>(), "Server received action entity");

            // If rebroadcast_inputs is enabled, set up replication to other clients
            if config.rebroadcast_inputs {
                debug!(action_entity = ?entity, "On server, insert ReplicateLike({:?}) for action entity ActionOf<{:?}>", wrapper.context, core::any::type_name::<C>());

                // TODO: don't rebroadcast to the original client
                commands.entity(entity).insert((
                    ReplicateLike {
                        root: wrapper.context,
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
    /// attach it to the correct [`Predicted`] context entity
    ///
    /// This cannot be a trigger because we need to wait until the Predicted entity is spawned
    #[cfg(feature = "client")]
    pub(crate) fn on_rebroadcast_action_received<C: Component>(
        query: Query<(Entity, &ActionOfWrapper<C>), (With<Replicated>, Without<ActionOf<C>>)>,
        context_query: Query<(Option<&Confirmed>, Has<DeterministicPredicted>), With<C>>,
        mut commands: Commands,
    ) {
        query.iter().for_each(|(entity, action_of_wrapper)| {
            if let Ok((confirmed, deterministic)) = context_query.get(action_of_wrapper.context)
                // context _entity is either the Predicted entity or DeterministicPredicted entity
                && let Some(context_entity) = confirmed.and_then(|c| c.predicted).or(deterministic.then_some(action_of_wrapper.context)) {
                debug!(?entity, "On client, insert ActionOf({:?}) for action entity ActionOf<{:?}> from input rebroadcast", context_entity, core::any::type_name::<C>());
                // Attach ActionOf to the predicted context entity
                commands
                    .entity(entity)
                    .insert((
                        ActionOf::<C>::new(context_entity),
                        // We add DeterministicPredicted because lightyear_inputs::client expects the recipient
                        // to be a predicted entity
                        DeterministicPredicted,
                        bevy_enhanced_input::context::ExternallyMocked,
                    ))
                    .remove::<ActionOfWrapper<C>>();
                // remove the Context from the confirmed entity, since the predicted entity is what matters
                if confirmed.is_some() && !deterministic {
                    commands.entity(action_of_wrapper.context).remove_with_requires::<C>();
                }
            } else {
                // we don't have a Predicted entity to attach this action to, just despawn
                commands.entity(entity)
                    .despawn();
            }
        });
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
