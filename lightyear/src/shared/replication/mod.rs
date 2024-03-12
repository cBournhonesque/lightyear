//! Module to handle replicating entities and components from server to client
use std::hash::Hash;

use anyhow::Result;
use bevy::ecs::component::Tick as BevyTick;
use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::{Component, Entity, Resource};
use bevy::reflect::Map;
use bevy::utils::HashSet;
use serde::{Deserialize, Serialize};

use crate::_reexport::{ComponentProtocol, ComponentProtocolKind};
use crate::channel::builder::Channel;
use crate::connection::netcode::ClientId;
use crate::packet::message::MessageId;
use crate::prelude::{NetworkTarget, Tick};
use crate::protocol::Protocol;
use crate::shared::replication::components::{Replicate, ReplicationGroupId};

pub mod components;

mod commands;
pub mod entity_map;
pub(crate) mod hierarchy;
pub mod metadata;
pub(crate) mod plugin;
pub(crate) mod receive;
pub(crate) mod send;
pub mod systems;

// // NOTE: cannot add trait bounds on C: ComponentProtocol and K: ComponentProtocolKind because of https://github.com/serde-rs/serde/issues/1296
// //  better to not add trait bounds on structs directly anyway
// #[cfg_attr(feature = "debug", derive(Debug))]
// #[derive(Serialize, Deserialize, Clone)]
// pub enum ReplicationMessage<C, K> {
//     // TODO: maybe include Vec<C> for SpawnEntity? All the components that already exist on this entity
//     SpawnEntity(Entity, Vec<C>),
//     DespawnEntity(Entity),
//     // TODO: maybe ComponentActions (Insert/Remove) in the same message? same logic, we might want to receive all of them at the same time
//     //  unfortunately can't really put entity-updates in the same message because it uses a different channel
//     /// All the components that are inserted on this entity
//     InsertComponent(Entity, Vec<C>),
//     /// All the components that are removed from this entity
//     RemoveComponent(Entity, Vec<K>),
//     // TODO: add the tick of the update? maybe this makes no sense if we gather updates only at the end of the tick
//     EntityUpdate(Entity, Vec<C>),
// }

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct EntityActions<C, K: Hash + Eq> {
    pub(crate) spawn: bool,
    pub(crate) despawn: bool,
    // Cannot use HashSet because we would need ComponentProtocol to implement Hash + Eq
    pub(crate) insert: Vec<C>,
    pub(crate) remove: HashSet<K>,
    // We also include the updates for the current tick in the actions, if there are any
    pub(crate) updates: Vec<C>,
}

impl<C, K: Hash + Eq> Default for EntityActions<C, K> {
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
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct EntityActionMessage<C, K: Hash + Eq> {
    sequence_id: MessageId,
    // we use vec but the order of entities should not matter
    pub(crate) actions: Vec<(Entity, EntityActions<C, K>)>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct EntityUpdatesMessage<C> {
    /// The last tick for which we sent an EntityActionsMessage for this group
    /// We set this to None after a certain amount of time without any new Actions, to signify on the receiver side
    /// that there is no ordering constraint with respect to Actions for this group (i.e. the Update can be applied immediately)
    last_action_tick: Option<Tick>,
    pub(crate) updates: Vec<(Entity, Vec<C>)>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum ReplicationMessageData<C, K: Hash + Eq> {
    /// All the entity actions (Spawn/despawn/inserts/removals) for a given group
    Actions(EntityActionMessage<C, K>),
    /// All the entity updates for a given group
    Updates(EntityUpdatesMessage<C>),
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ReplicationMessage<C, K: Hash + Eq> {
    pub(crate) group_id: ReplicationGroupId,
    pub(crate) data: ReplicationMessageData<C, K>,
}

#[doc(hidden)]
/// Trait for any service that can send replication messages to the remote.
/// (this trait is used to easily enable both client to server and server to client replication)
///
/// The trait is made public because it is needed in the macros
pub trait ReplicationSend<P: Protocol>: Resource {
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
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()>;

    fn prepare_entity_despawn(
        &mut self,
        entity: Entity,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()>;

    fn prepare_component_insert(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        // bevy_tick for the current system run (we send component updates since the most recent bevy_tick of
        //  last update ack OR last action sent)
        system_current_tick: BevyTick,
    ) -> Result<()>;

    fn prepare_component_remove(
        &mut self,
        entity: Entity,
        component_kind: P::ComponentKinds,
        replicate: &Replicate<P>,
        target: NetworkTarget,
        system_current_tick: BevyTick,
    ) -> Result<()>;

    fn prepare_entity_update(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate<P>,
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

    fn get_mut_replicate_component_cache(&mut self) -> &mut EntityHashMap<Replicate<P>>;

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
