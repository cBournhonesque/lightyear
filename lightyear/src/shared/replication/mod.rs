//! Module to handle replicating entities and components from server to client
use crate::_reexport::{ComponentProtocol, ComponentProtocolKind};
use anyhow::Result;
use bevy::prelude::{Component, Entity, Resource};
use bevy::reflect::Map;
use serde::{Deserialize, Serialize};

use crate::channel::builder::{Channel, EntityActionsChannel, EntityUpdatesChannel};
use crate::netcode::ClientId;
use crate::prelude::{EntityMap, MapEntities};
use crate::protocol::channel::ChannelKind;
use crate::protocol::Protocol;
use crate::shared::replication::components::Replicate;

pub mod components;

pub mod entity_map;

pub mod manager;

pub mod resources;

pub mod systems;

// NOTE: cannot add trait bounds on C: ComponentProtocol and K: ComponentProtocolKind because of https://github.com/serde-rs/serde/issues/1296
//  better to not add trait bounds on structs directly anyway
#[cfg_attr(feature = "debug", derive(Debug))]
#[derive(Serialize, Deserialize, Clone)]
pub enum ReplicationMessage<C, K> {
    // TODO: maybe include Vec<C> for SpawnEntity? All the components that already exist on this entity
    SpawnEntity(Entity, Vec<C>),
    DespawnEntity(Entity),
    InsertComponent(Entity, C),
    RemoveComponent(Entity, K),
    // TODO: add the tick of the update? maybe this makes no sense if we gather updates only at the end of the tick
    EntityUpdate(Entity, Vec<C>),
}

impl<C: MapEntities, K: MapEntities> MapEntities for ReplicationMessage<C, K> {
    fn map_entities(&mut self, entity_map: &EntityMap) {
        match self {
            ReplicationMessage::SpawnEntity(e, components) => {
                e.map_entities(entity_map);
                for component in components {
                    component.map_entities(entity_map);
                }
            }
            ReplicationMessage::DespawnEntity(e) => {
                e.map_entities(entity_map);
            }
            ReplicationMessage::InsertComponent(e, c) => {
                e.map_entities(entity_map);
                c.map_entities(entity_map);
            }
            ReplicationMessage::RemoveComponent(e, k) => {
                e.map_entities(entity_map);
                k.map_entities(entity_map);
            }
            ReplicationMessage::EntityUpdate(e, components) => {
                e.map_entities(entity_map);
                for component in components {
                    component.map_entities(entity_map);
                }
            }
        }
    }
}

pub trait ReplicationSend<P: Protocol>: Resource {
    /// Return the list of clients that connected to the server since we last sent any replication messages
    /// (this is used to send the initial state of the world to new clients)
    fn new_remote_peers(&self) -> Vec<ClientId>;

    fn entity_spawn(
        &mut self,
        entity: Entity,
        components: Vec<P::Components>,
        replicate: &Replicate,
    ) -> Result<()>;

    fn entity_despawn(&mut self, entity: Entity, replicate: &Replicate) -> Result<()>;

    fn component_insert(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate,
    ) -> Result<()>;

    fn component_remove(
        &mut self,
        entity: Entity,
        component_kind: P::ComponentKinds,
        replicate: &Replicate,
    ) -> Result<()>;

    fn entity_update_single_component(
        &mut self,
        entity: Entity,
        component: P::Components,
        replicate: &Replicate,
    ) -> Result<()>;

    fn entity_update(
        &mut self,
        entity: Entity,
        components: Vec<P::Components>,
        replicate: &Replicate,
    ) -> Result<()>;

    /// Any operation that needs to happen before we can send the replication messages
    /// (for example collecting the individual single component updates into a single message)
    fn prepare_replicate_send(&mut self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use crate::prelude::client::*;
    use crate::prelude::*;
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};
    use std::time::Duration;

    // An entity gets replicated from server to client,
    // then a component gets removed from that entity on server,
    // that component should also removed on client as well.
    #[test]
    fn test_simple_component_remove() -> anyhow::Result<()> {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            enable_replication: true,
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
        stepper.client_mut().connect();
        stepper.client_mut().set_synced();

        // Advance the world to let the connection process complete
        for _ in 0..20 {
            stepper.frame_step();
        }

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
            .client()
            .connection()
            .base()
            .replication_manager
            .entity_map
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
