use bevy_app::App;
use bevy_ecs::prelude::*;
use bevy_ecs::relationship::Relationship;
use bevy_enhanced_input::context::ExternallyMocked;
use bevy_utils::prelude::DebugName;
#[cfg(feature = "client")]
use lightyear_replication::prelude::Replicate;

use bevy_enhanced_input::prelude::*;
use lightyear_connection::client::Client;
use lightyear_connection::host::HostClient;
use lightyear_link::prelude::Server;
use lightyear_replication::prelude::*;
use lightyear_serde::SerializationError;
use lightyear_serde::registry::SerializeFns;
use lightyear_serde::writer::Writer;
#[allow(unused_imports)]
use tracing::{debug, info};
#[cfg(feature = "server")]
use {
    lightyear_connection::host::HostServer, lightyear_inputs::server::ServerInputConfig,
    lightyear_replication::prelude::ReplicateLike,
};
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
    /// For Host-Server, if an ActionOf is spawned directly on the HostClient.
    /// (without being Replicated, or with Prespawned)
    /// Then we initiate rebroadcast
    #[cfg(all(feature = "client", feature = "server"))]
    pub(crate) fn add_action_of_host_server_rebroadcast<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        host_server: Single<(), With<HostServer>>,
        action: Query<&ActionOf<C>, Or<(Without<Replicated>, With<PreSpawned>)>>,
        mut commands: Commands,
    ) {
        let entity = trigger.entity;
        if let Ok(action_of) = action.get(entity) {
            let context_entity = action_of.get();
            debug!(action_entity = ?entity, "Replicating ActionOf<{:?}> for context entity {context_entity:?} from HostClient to other clients for input rebroadcast", DebugName::type_name::<C>());
            commands.entity(entity).insert((ReplicateLike {
                root: context_entity,
            },));
        }
    }

    /// When an [`ActionOf<C>`] component is added to an entity (usually on the client),
    /// we add Replicate to it so that the action entity is also created on the server.
    ///
    /// PreSpawned Actions must be replicated from server to client.
    /// No need to change anything about ActionOf because the Context and Action will be received at the same time,
    /// so the entity mapping in ActionOf will work properly.
    #[cfg(feature = "client")]
    pub(crate) fn add_action_of_replicate<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        server: Query<(), With<Server>>,
        // we don't want to add Replicate on action entities that were already replicated
        // PreSpawned entities are replicated from server to client
        action: Query<&ActionOf<C>, (Without<Replicated>, Without<PreSpawned>)>,
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
        _: Single<(), (With<Server>, Without<HostServer>)>,
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
        }
    }

    /// When the client receives a rebroadcast Action entity with [`ReplicateLike`],
    ///
    /// Attach ExternallyMocked to it to signify that the ActionState should only be updated
    /// from rebroadcasted input messages. (in particular, BEI doesn't tick the time for those actions)
    #[cfg(feature = "client")]
    pub(crate) fn on_rebroadcast_action_received<C: Component>(
        trigger: On<Add, ActionOf<C>>,
        single: Single<(), (With<Client>, Without<HostClient>)>,
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

            commands.entity(entity).insert(
                // Make sure that the actions are only updated via input messages
                ExternallyMocked,
            );
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
