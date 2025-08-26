//! For BEI, option 1:
//! - Server spawns Context entity
//! - Client spawns ActionOf entities with Action<A> components + Bindings
//!   (which inserts all other required components, such as ActionSettings, ActionFns)
//!   ActionFns is needed to trigger events
//!
//! Then:
//! - Client sends an initial message to the server that contains
//!   - for a given context, a vec of (Entity, kind) where kind is the type of InputAction
//!
//! Alternative:
//! - client inserts Replicate on the ActionOf entities, but only ActionOf component is replicated from client -> server (which should be ok since other components won't be in the protocol)
//!
//! Option 2:
//! - shared system to spawn context + actions on both client and server, and we need to perform entity mapping.

use bevy_app::App;
use bevy_ecs::entity::MapEntities;
use bevy_ecs::prelude::*;
#[cfg(feature = "client")]
use bevy_ecs::relationship::Relationship;
#[cfg(feature = "client")]
use lightyear_replication::prelude::Replicate;

use lightyear_replication::prelude::Replicated;
use lightyear_connection::prelude::NetworkTarget;
use bevy_enhanced_input::prelude::*;
#[cfg(feature = "client")]
use lightyear_core::prediction::Predicted;
#[cfg(feature = "server")]
use lightyear_inputs::config::InputConfig;
use lightyear_replication::prelude::{AppComponentExt, ComponentReplicationConfig, PredictionTarget, InterpolationTarget};
#[cfg(feature = "server")]
use lightyear_replication::send::components::ReplicationMode;
use lightyear_serde::SerializationError;
use lightyear_serde::registry::SerializeFns;
use lightyear_serde::writer::Writer;
use serde::{Deserialize, Serialize};
use tracing::info;

/// Wrapper around ActionOf<C> that is needed for replication with custom entity mapping
#[derive(Component, Serialize, Deserialize)]
pub(crate) struct ActionOfWrapper<C> {
    context: Entity,
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

/// Component that tells Action entities to copy replication settings from their Context entity
#[derive(Component, Serialize, Deserialize, Clone, PartialEq)]
pub(crate) struct ReplicateLike<C> {
    pub context: Entity,
    pub marker: core::marker::PhantomData<C>,
}

impl<C> ReplicateLike<C> {
    pub fn new(context: Entity) -> Self {
        Self {
            context,
            marker: core::marker::PhantomData,
        }
    }
}

impl<C> MapEntities for ReplicateLike<C> {
    fn map_entities<E: EntityMapper>(&mut self, entity_mapper: &mut E) {
        self.context = entity_mapper.get_mapped(self.context);
    }
}

pub struct InputRegistryPlugin;

impl InputRegistryPlugin {
    /// When an ActionOf<C> component is added to an entity (usually on the client),
    /// we add Replicate to it so that the action entity is also created on the server.
    #[cfg(feature = "client")]
    pub(crate) fn add_action_of_replicate<C: Component>(
        trigger: Trigger<OnAdd, ActionOf<C>>,
        // we don't want to add Replicate on action entities that were already replicated
        action: Query<&ActionOf<C>, Without<Replicated>>,
        query: Query<Option<&Predicted>>,
        mut commands: Commands,
    ) {
        if let Ok(action_of) = action.get(trigger.target())
            && let Ok(predicted) = query.get(action_of.get())
        {
            // TODO: remove ActionOfWrapper after the first replication?
            if let Some(predicted) = predicted {
                commands.entity(trigger.target()).insert((
                    // we replicate using the confirmed entity so that the server can map it to the server entity
                    ActionOfWrapper::<C>::new(predicted.confirmed_entity.unwrap()),
                    Replicate::to_server(),
                ));
            } else {
                commands.entity(trigger.target()).insert((
                    // we replicate using the confirmed entity so that the server can map it to the server entity
                    ActionOfWrapper::<C>::new(action_of.get()),
                    Replicate::to_server(),
                ));
            }
        }
    }

    /// When the server receives ActionOfWrapper, insert the appropriate ActionOf
    /// and optionally rebroadcast to other clients if rebroadcast_inputs is enabled
    #[cfg(feature = "server")]
    pub(crate) fn on_action_of_replicated<C: Component>(
        trigger: Trigger<OnAdd, ActionOfWrapper<C>>,
        query: Query<&ActionOfWrapper<C>, With<Replicated>>,
        config: Res<InputConfig<C>>,
        // Query to get replication settings from the context entity
        context_query: Query<(
            Option<&Replicate>,
            Option<&PredictionTarget>,
            Option<&InterpolationTarget>,
        )>,
        mut commands: Commands,
    ) {
        let entity = trigger.target();
        if let Ok(wrapper) = query.get(entity) {
            commands
                .entity(entity)
                .insert(ActionOf::<C>::new(wrapper.context))
                .remove::<ActionOfWrapper<C>>();

            // If rebroadcast_inputs is enabled, set up replication to other clients
            if config.rebroadcast_inputs {
                // Get replication settings from the context entity
                if let Ok((replicate, prediction_target, interpolation_target)) =
                    context_query.get(wrapper.context)
                {
                    // Copy replication settings from context entity, but exclude the original sender
                    if let Some(replicate) = replicate {
                        // Extract the network target from the ReplicationMode
                        let modified_replicate = if let ReplicationMode::SingleServer(target) = &replicate.mode {
                            match target {
                                NetworkTarget::All => Replicate::to_clients(NetworkTarget::All),
                                NetworkTarget::AllExceptSingle(excluded) => {
                                    Replicate::to_clients(NetworkTarget::AllExceptSingle(*excluded))
                                }
                                NetworkTarget::Single(client) => {
                                    // Don't rebroadcast to the sender
                                    Replicate::to_clients(NetworkTarget::AllExceptSingle(*client))
                                }
                                _ => Replicate::to_clients(NetworkTarget::All),
                            }
                        } else {
                            // Default fallback
                            Replicate::to_clients(NetworkTarget::All)
                        };
                        commands.entity(entity).insert(modified_replicate);
                    }

                    // Copy prediction target settings
                    if let Some(prediction_target) = prediction_target {
                        commands.entity(entity).insert(prediction_target.clone());
                    }

                    // Copy interpolation target settings
                    if let Some(interpolation_target) = interpolation_target {
                        commands.entity(entity).insert(interpolation_target.clone());
                    }

                    // Add ReplicateLike marker to help clients map to correct context
                    commands.entity(entity).insert(ReplicateLike::<C>::new(wrapper.context));
                }
            }
        }
    }

    /// When the client receives a rebroadcast Action entity with ReplicateLike,
    /// attach it to the correct Predicted context entity
    #[cfg(feature = "client")]
    pub(crate) fn on_rebroadcast_action_received<C: Component>(
        trigger: Trigger<OnAdd, ReplicateLike<C>>,
        query: Query<&ReplicateLike<C>, With<Replicated>>,
        // Query to find the predicted entity for the mapped context
        context_query: Query<Entity, (With<Predicted>, With<C>)>,
        mut commands: Commands,
    ) {
        let entity = trigger.target();
        if let Ok(replicate_like) = query.get(entity) {
            // Find the predicted entity that corresponds to the mapped context
            // The replicate_like.context should already be mapped to the predicted entity
            if context_query.get(replicate_like.context).is_ok() {
                // Attach ActionOf to the predicted context entity
                commands
                    .entity(entity)
                    .insert(ActionOf::<C>::new(replicate_like.context))
                    .remove::<ReplicateLike<C>>();

                info!(
                    "Attached rebroadcast Action entity {:?} to predicted context {:?}",
                    entity, replicate_like.context
                );
            } else {
                // If we can't find the predicted entity, log a warning
                info!(
                    "Could not find predicted context entity {:?} for rebroadcast Action {:?}",
                    replicate_like.context, entity
                );
            }
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
