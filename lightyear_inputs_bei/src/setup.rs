use bevy::prelude::Reflect;
use bevy_app::App;
use bevy_ecs::entity::MapEntities;
use bevy_ecs::prelude::*;
use bevy_ecs::relationship::Relationship;
use bevy_utils::prelude::DebugName;
#[cfg(feature = "client")]
use lightyear_replication::prelude::Replicate;

use bevy_enhanced_input::prelude::*;
use serde::{Deserialize, Serialize};
use lightyear_link::prelude::Server;
#[cfg(feature = "client")]
use lightyear_prediction::prelude::DeterministicPredicted;
use lightyear_replication::prelude::*;
use lightyear_serde::SerializationError;
use lightyear_serde::registry::SerializeFns;
use lightyear_serde::writer::Writer;
#[allow(unused_imports)]
use tracing::{debug, info};
#[cfg(feature = "server")]
use {lightyear_inputs::server::ServerInputConfig, lightyear_replication::prelude::ReplicateLike};


// TODO: ideally we would have an entity-mapped that is PreSpawn aware. If you include an entity
//   that is PreSpawned, then in the entity-mapper we use a Query<Entity, With<PreSpawned>> to check the hash
//   of the entity and serialize it as the hash. Then the receiving entity mapper could look up the corresponding
//   entity by the PreSpawn hash to apply entity mapping.
//   1. In common case, server sends P1,C1. It does NOT need to change ChildOf(P1) because client will match P1/C1 on receipt, then
//        update its entity maps, then the component map entity will work correctly. We just need to make sure that C1 is also Prespawned,
//        which we could do in ReplicateLike Propagation? (but how to do it on the receiver side?)
//


pub struct InputRegistryPlugin;

impl InputRegistryPlugin {
    /// When an [`ActionOf<C>`] component is added to an entity (usually on the client),
    /// we add Replicate to it so that the action entity is also created on the server.
    ///
    /// How do we handle `PreSpawned` Context or Action entities?
    /// 1. Server and Client prespawn a context entity C on tick T.
    /// 2. Client spawns an action entity A that it replicates to server (tick T). It has ActionOfWrapper(C)
    ///    which contains the hash of context entity C.
    /// 3. Client starts sending input messages that mention entity A. Messages that arrive on client before tick T
    ///    contain an unknown entity and will be ignored. As soon as the server receives the entity A, the entity mapping
    ///    will work on the server side and the messages will be applied correctly.
    /// 4. Server receives the entity A, it applies ActionOf(C) correctly by looking at ActionOfWrapper(C)
    ///    + the PreSpawned values. Safer to use Query<&PreSpawned> than &PreSpawnedReceiver since the latter
    ///    only works for entities that don't have Replicate, but the server is the one that replicates C.
    ///
    /// The only issue is that the entity could be replicated too late, we want to be able to replicate
    /// an entity as soon as possible, or possibly even in the same packet as the InputMessage.
    /// One option would be to update the InputMessage with a special PreSpawnBEI variant that contains:
    /// - the hash of the context entity (for prespawning),
    /// - the entity of the action entity (so that the server spawns the action entity and updates its entity mapping)
    /// This is so that the server spawns the action entity as soon as possible (even before the tick T), so that the
    /// server and client have the same inputs and predict the same movement.
    #[cfg(feature = "client")]
    pub(crate) fn add_action_of_replicate<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        server: Query<(), With<Server>>,
        // we don't want to add Replicate on action entities that were already replicated
        action: Query<&ActionOf<C>, Without<Replicated>>,
        context: Query<Option<&PreSpawned>, With<C>>,
        mut commands: Commands,
    ) {
        if server.single().is_ok() {
            // we're on the server, don't do anything
            return;
        }
        let entity = trigger.entity;
        if let Ok(action_of) = action.get(entity) && let Ok(prespawned) = context.get(action_of.get()) {
            let context_entity = action_of.get();
            debug!(action_entity = ?entity, "Replicating ActionOf<{:?}> for context entity {context_entity:?} from client to server", DebugName::type_name::<C>());
            if let Some(prespawned) = prespawned {
                // we need to replicate the action entity as a prespawned entity
                // We include the information directly in the InputMessage so that the server is aware of the
                // Action entity as soon as possible (ideally before it even spawns the Context entity)
            } else {
                commands.entity(entity).insert((Replicate::to_server(),));
            }
        }
    }

    /// When the server receives [`ActionOf`], optionally rebroadcast to other clients if rebroadcast_inputs is enabled
    #[cfg(feature = "server")]
    pub(crate) fn on_action_of_replicated<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        query: Query<&ActionOf<C>, With<Replicated>>,
        _: Single<(), With<Server>>,
        config: Res<ServerInputConfig<C>>,
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
