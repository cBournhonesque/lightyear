//! Module to handle replicating entities and components from server to client
use std::fmt::Debug;
use std::hash::Hash;

use anyhow::Result;
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::{Component, Entity, Resource};
use bevy::reflect::Map;
use bevy::utils::HashSet;
use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use crate::_reexport::{
    ComponentProtocol, ComponentProtocolKind, IterComponentInsertEvent, IterComponentRemoveEvent,
    IterComponentUpdateEvent, WriteWordBuffer,
};
use crate::channel::builder::Channel;
use crate::connection::id::ClientId;
use crate::packet::message::MessageId;
use crate::prelude::{NetworkTarget, Tick};
use crate::protocol::component::{ComponentNetId, ComponentRegistry};
use crate::protocol::registry::NetId;
use crate::protocol::{EventContext, Protocol};
use crate::serialize::RawData;
use crate::shared::replication::components::{Replicate, ReplicationGroupId};

pub mod components;

mod commands;
pub mod entity_map;
pub(crate) mod hierarchy;
pub(crate) mod plugin;
pub(crate) mod receive;
pub(crate) mod resources;
pub(crate) mod send;
pub mod systems;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub struct EntityActions {
    pub(crate) spawn: bool,
    pub(crate) despawn: bool,
    // Cannot use HashSet because we would need ComponentProtocol to implement Hash + Eq
    pub(crate) insert: Vec<RawData>,
    #[bitcode(with_serde)]
    // TODO: use a ComponentNetId instead of NetId?
    pub(crate) remove: HashSet<NetId>,
    // We also include the updates for the current tick in the actions, if there are any
    pub(crate) updates: Vec<RawData>,
}

impl Default for EntityActions {
    fn default() -> Self {
        Self {
            spawn: false,
            despawn: false,
            insert: Vec::new(),
            remove: HashSet::new(),
            updates: Vec::new(),
        }
    }
}

// TODO: 99% of the time the ReplicationGroup is the same as the Entity in the hashmap, and there's only 1 entity
//  have an optimization for that
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub struct EntityActionMessage {
    sequence_id: MessageId,
    #[bitcode(with_serde)]
    // we use vec but the order of entities should not matter
    pub(crate) actions: Vec<(Entity, EntityActions)>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub struct EntityUpdatesMessage {
    /// The last tick for which we sent an EntityActionsMessage for this group
    /// We set this to None after a certain amount of time without any new Actions, to signify on the receiver side
    /// that there is no ordering constraint with respect to Actions for this group (i.e. the Update can be applied immediately)
    last_action_tick: Option<Tick>,
    #[bitcode(with_serde)]
    pub(crate) updates: Vec<(Entity, Vec<RawData>)>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub enum ReplicationMessageData {
    /// All the entity actions (Spawn/despawn/inserts/removals) for a given group
    Actions(EntityActionMessage),
    /// All the entity updates for a given group
    Updates(EntityUpdatesMessage),
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Encode, Decode)]
pub struct ReplicationMessage {
    pub(crate) group_id: ReplicationGroupId,
    pub(crate) data: ReplicationMessageData,
}

#[doc(hidden)]
/// Trait for any service that can send replication messages to the remote.
/// (this trait is used to easily enable both client to server and server to client replication)
///
/// The trait is made public because it is needed in the macros
pub trait ReplicationSend: Resource {
    type Events: IterComponentInsertEvent<Self::EventContext>
        + IterComponentRemoveEvent<Self::EventContext>
        + IterComponentUpdateEvent<Self::EventContext>;
    // Type of the context associated with the events emitted by this replication plugin
    type EventContext: EventContext;
    /// Marker to identify the type of the ReplicationSet component
    /// This is mostly relevant in the unified mode, where a ReplicationSet can be added several times
    /// (in the client and the server replication plugins)
    type SetMarker: Debug + Hash + Send + Sync + Eq + Clone;

    fn events(&mut self) -> &mut Self::Events;

    fn writer(&mut self) -> &mut WriteWordBuffer;

    fn component_registry(&self) -> &ComponentRegistry;

    /// Set the priority for a given replication group, for a given client
    /// This IS the client-facing API that users must use to update the priorities for a given client.
    ///
    /// If multiple entities in the group have different priorities, then the latest updated priority will take precedence
    fn update_priority(
        &mut self,
        replication_group_id: ReplicationGroupId,
        client_id: ClientId,
        priority: f32,
    ) -> Result<()>;

    /// Return the list of clients that connected to the server since we last sent any replication messages
    /// (this is used to send the initial state of the world to new clients)
    fn new_connected_clients(&self) -> Vec<ClientId>;

    fn prepare_entity_spawn(
        &mut self,
        entity: Entity,
        replicate: &Replicate,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()>;

    fn prepare_entity_despawn(
        &mut self,
        entity: Entity,
        replicate: &Replicate,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()>;

    fn prepare_component_insert(
        &mut self,
        entity: Entity,
        kind: ComponentNetId,
        component: RawData,
        replicate: &Replicate,
        target: NetworkTarget,
        // bevy_tick for the current system run (we send component updates since the most recent bevy_tick of
        //  last update ack OR last action sent)
        system_current_tick: BevyTick,
    ) -> Result<()>;

    fn prepare_component_remove(
        &mut self,
        entity: Entity,
        component_kind: ComponentNetId,
        replicate: &Replicate,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()>;

    fn prepare_component_update(
        &mut self,
        entity: Entity,
        kind: ComponentNetId,
        component: RawData,
        replicate: &Replicate,
        target: NetworkTarget,
        // bevy_tick when the component changes
        component_change_tick: BevyTick,
        // bevy_tick for the current system run
        system_current_tick: BevyTick,
    ) -> Result<()>;

    /// Any operation that needs to happen before we can send the replication messages
    /// (for example collecting the individual single component updates into a single message,
    ///
    /// Similarly, we want to collect all ComponentInsert and ComponentRemove into a single message.
    /// Why? Because if we create separate message for each ComponentInsert (for example when the entity gets spawned)
    /// Then those 2 component inserts might be stored in different packets, and arrive at different times because of jitter
    ///
    /// But the receiving systems might expect both components to be present at the same time.
    fn buffer_replication_messages(&mut self, tick: Tick, bevy_tick: BevyTick) -> Result<()>;

    fn get_mut_replicate_component_cache(&mut self) -> &mut EntityHashMap<Replicate>;

    /// Do some regular cleanup on the internals of replication
    /// - account for tick wrapping by resetting some internal ticks for each replication group
    fn cleanup(&mut self, tick: Tick);
}

#[cfg(test)]
mod tests {
    use bevy::utils::Duration;

    use crate::prelude::client::*;
    use crate::prelude::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};

    // An entity gets replicated from server to client,
    // then a component gets removed from that entity on server,
    // that component should also removed on client as well.
    #[test]
    fn test_simple_component_remove() -> anyhow::Result<()> {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(0),
            incoming_jitter: Duration::from_millis(0),
            incoming_loss: 0.0,
        };
        let sync_config = SyncConfig::default().speedup_factor(1.0);
        let prediction_config = PredictionConfig::default().disable(false);
        let interpolation_config = InterpolationConfig::default();
        let mut stepper = BevyStepper::new(
            shared_config,
            sync_config,
            prediction_config,
            interpolation_config,
            link_conditioner,
            frame_duration,
        );
        stepper.init();

        // Create an entity on server
        let server_entity = stepper
            .server_app
            .world
            .spawn((Component1(0.0), Replicate::default()))
            .id();
        // we need to step twice because we run client before server
        stepper.frame_step();
        stepper.frame_step();

        // Check that the entity is replicated to client
        let client_entity = *stepper
            .client_app
            .world
            .resource::<ClientConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .unwrap();
        assert_eq!(
            stepper
                .client_app
                .world
                .entity(client_entity)
                .get::<Component1>()
                .unwrap(),
            &Component1(0.0)
        );

        // Remove the component on the server
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .remove::<Component1>();
        stepper.frame_step();
        stepper.frame_step();

        // Check that this removal was replicated
        assert!(stepper
            .client_app
            .world
            .entity(client_entity)
            .get::<Component1>()
            .is_none());
        Ok(())
    }
}
