use std::collections::HashMap;
use bevy_ecs::component::ComponentId;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::shared::{serde::{BitReader, Serde, SerdeErr, UnsignedVariableInteger}, Components,
                    EntityAction, EntityActionReceiver, EntityActionType,
                    MessageIndex, NetEntity,  ReplicateSafe,
                    Tick, NetEntityConverter};

use crate::client::Events;

use super::entity_record::EntityRecord;

pub struct EntityManager {
    entity_records: HashMap<Entity, EntityRecord>,
    local_to_world_entity: HashMap<NetEntity, Entity>,
    receiver: EntityActionReceiver,
    received_components: HashMap<(NetEntity, ComponentId), Box<dyn ReplicateSafe>>,
}

impl Default for EntityManager {
    fn default() -> Self {
        Self {
            entity_records: HashMap::default(),
            local_to_world_entity: HashMap::default(),
            receiver: EntityActionReceiver::default(),
            received_components: HashMap::default(),
        }
    }
}

impl EntityManager {
    // Action Reader
    fn read_message_id(
        reader: &mut BitReader,
        last_id_opt: &mut Option<MessageIndex>,
    ) -> Result<MessageIndex, SerdeErr> {
        let current_id = if let Some(last_id) = last_id_opt {
            // read diff
            let id_diff = UnsignedVariableInteger::<3>::de(reader)?.get() as MessageIndex;
            last_id.wrapping_add(id_diff)
        } else {
            // read message id
            MessageIndex::de(reader)?
        };
        *last_id_opt = Some(current_id);
        Ok(current_id)
    }

    /// Read and process incoming actions.
    ///
    /// * Emits client events corresponding to any [`EntityAction`] received
    /// Store
    pub fn read_actions(
        &mut self,
        world: &mut World,
        reader: &mut BitReader,
        incoming_events: &mut Events,
    ) -> Result<(), SerdeErr> {
        let mut last_read_id: Option<MessageIndex> = None;

        loop {
            // read action continue bit
            let action_continue = bool::de(reader)?;
            if !action_continue {
                break;
            }

            self.read_action(reader, &mut last_read_id)?;
        }

        self.process_incoming_actions(world, incoming_events);

        Ok(())
    }

    /// Read the bits corresponding to the EntityAction and adds the [`EntityAction`]
    /// to an internal buffer.
    ///
    /// We can use a UnorderedReliableReceiver buffer because the messages have already been
    /// ordered by the client's jitter buffer
    fn read_action(
        &mut self,
        reader: &mut BitReader,
        last_read_id: &mut Option<MessageIndex>,
    ) -> Result<(), SerdeErr> {
        let action_id = Self::read_message_id(reader, last_read_id)?;

        let action_type = EntityActionType::de(reader)?;

        match action_type {
            // Entity Creation
            EntityActionType::SpawnEntity => {
                // read entity
                let net_entity = NetEntity::de(reader)?;

                // read components
                let components_num = UnsignedVariableInteger::<3>::de(reader)?.get();
                let mut component_kinds = Vec::new();
                for _ in 0..components_num {
                    let new_component = Components::read(reader, self)?;
                    let new_component_kind = new_component.dyn_ref().kind();
                    self.received_components
                        .insert((net_entity, new_component_kind), new_component);
                    component_kinds.push(new_component_kind);
                }

                self.receiver.buffer_action(
                    action_id,
                    EntityAction::SpawnEntity(net_entity, component_kinds),
                );
            }
            // Entity Deletion
            EntityActionType::DespawnEntity => {
                // read all data
                let net_entity = NetEntity::de(reader)?;

                self.receiver
                    .buffer_action(action_id, EntityAction::DespawnEntity(net_entity));
            }
            // Add Component to Entity
            EntityActionType::InsertComponent => {
                // read all data
                let net_entity = NetEntity::de(reader)?;
                let new_component = Components::read(reader, self)?;
                let new_component_kind = new_component.dyn_ref().kind();

                self.receiver.buffer_action(
                    action_id,
                    EntityAction::InsertComponent(net_entity, new_component_kind),
                );
                self.received_components
                    .insert((net_entity, new_component_kind), new_component);
            }
            // Component Removal
            EntityActionType::RemoveComponent => {
                // read all data
                let net_entity = NetEntity::de(reader)?;
                let component_kind = ComponentId::de(reader)?;

                self.receiver.buffer_action(
                    action_id,
                    EntityAction::RemoveComponent(net_entity, component_kind),
                );
            }
            EntityActionType::Noop => {
                self.receiver.buffer_action(action_id, EntityAction::Noop);
            }
        }

        Ok(())
    }

    /// For each [`EntityAction`] that can be executed now,
    /// execute it and emit a corresponding event.
    fn process_incoming_actions(
        &mut self,
        world: &mut World,
        incoming_events: &mut Events,
    ) {
        // receive the list of EntityActions that can be executed now
        let incoming_actions = self.receiver.receive_actions();

        // execute the action and emit an event
        for action in incoming_actions {
            match action {
                EntityAction::SpawnEntity(net_entity, components) => {
                    //let e_u16: u16 = net_entity.into();
                    //info!("spawn entity: {}", e_u16);

                    if self.local_to_world_entity.contains_key(&net_entity) {
                        panic!("attempted to insert duplicate entity");
                    }

                    // set up entity
                    let world_entity = world.spawn_entity();
                    self.local_to_world_entity.insert(net_entity, world_entity);
                    let entity_handle = self.handle_entity_map.insert(world_entity);
                    let mut entity_record = EntityRecord::new(net_entity, entity_handle);

                    incoming_events.push_spawn(world_entity);

                    // read component list
                    for component_kind in components {
                        let component = self
                            .received_components
                            .remove(&(net_entity, component_kind))
                            .unwrap();

                        entity_record.component_kinds.insert(component_kind);

                        //component.extract_and_insert(&world_entity, world);
                        todo!();

                        incoming_events.push_insert(world_entity, component_kind);
                    }
                    //

                    self.entity_records.insert(world_entity, entity_record);
                }
                EntityAction::DespawnEntity(net_entity) => {
                    //let e_u16: u16 = net_entity.into();
                    //info!("despawn entity: {}", e_u16);

                    if let Some(world_entity) = self.local_to_world_entity.remove(&net_entity) {
                        if self.entity_records.remove(&world_entity).is_none() {
                            panic!("despawning an uninitialized entity");
                        }

                        // Generate event for each component, handing references off just in
                        // case
                        for component_kind in world.component_kinds(&world_entity) {
                            if let Some(component) =
                                world.remove_component_of_kind(&world_entity, &component_kind)
                            {
                                incoming_events.push_remove(world_entity, component);
                            }
                        }

                        world.despawn_entity(&world_entity);

                        incoming_events.push_despawn(world_entity);
                    } else {
                        panic!("received message attempting to delete nonexistent entity");
                    }
                }
                EntityAction::InsertComponent(net_entity, component_kind) => {
                    //let e_u16: u16 = net_entity.into();
                    //info!("insert component for: {}", e_u16);

                    let component = self
                        .received_components
                        .remove(&(net_entity, component_kind))
                        .unwrap();

                    if !self.local_to_world_entity.contains_key(&net_entity) {
                        panic!(
                            "attempting to add a component to nonexistent entity: {}",
                            Into::<u16>::into(net_entity)
                        );
                    } else {
                        let world_entity = self.local_to_world_entity.get(&net_entity).unwrap();

                        let entity_record = self.entity_records.get_mut(world_entity).unwrap();

                        entity_record.component_kinds.insert(component_kind);

                        //component.extract_and_insert(world_entity, world);
                        todo!();

                        incoming_events.push_insert(*world_entity, component_kind);
                    }
                }
                EntityAction::RemoveComponent(net_entity, component_kind) => {
                    //let e_u16: u16 = net_entity.into();
                    //info!("remove component for: {}", e_u16);

                    let world_entity = self
                        .local_to_world_entity
                        .get_mut(&net_entity)
                        .expect("attempting to delete component of nonexistent entity");
                    let entity_record = self
                        .entity_records
                        .get_mut(world_entity)
                        .expect("attempting to delete component of nonexistent entity");
                    if entity_record.component_kinds.remove(&component_kind) {
                        // Get component for last change
                        let component = world
                            .remove_component_of_kind(world_entity, &component_kind)
                            .expect("Component already removed?");

                        // Generate event
                        incoming_events.push_remove(*world_entity, component);
                    } else {
                        panic!("attempting to delete nonexistent component of entity");
                    }
                }
                EntityAction::Noop => {
                    // do nothing
                }
            }
        }
    }

    /// Read component updates from raw bits
    pub fn read_updates(
        &mut self,
        world: &mut World,
        server_tick: Tick,
        reader: &mut BitReader,
        incoming_events: &mut Events,
    ) -> Result<(), SerdeErr> {
        loop {
            // read update continue bit
            let update_continue = bool::de(reader)?;
            if !update_continue {
                break;
            }

            let net_entity_id = NetEntity::de(reader)?;

            self.read_update(world, server_tick, reader, &net_entity_id, incoming_events)?;
        }

        Ok(())
    }

    /// Read component updates from raw bits for a given entity
    fn read_update(
        &mut self,
        world: &mut World,
        server_tick: Tick,
        reader: &mut BitReader,
        net_entity_id: &NetEntity,
        incoming_events: &mut Events,
    ) -> Result<(), SerdeErr> {
        loop {
            // read update continue bit
            let component_continue = bool::de(reader)?;
            if !component_continue {
                break;
            }

            let component_update = Components::read_create_update(reader)?;
            let component_kind = component_update.kind;

            if let Some(world_entity) = self.local_to_world_entity.get(&net_entity_id) {
                world.component_apply_update(
                    self,
                    world_entity,
                    &component_kind,
                    component_update,
                )?;

                incoming_events.push_update(server_tick, *world_entity, component_kind);
            }
        }

        Ok(())
    }
}

impl NetEntityConverter for EntityManager {
    fn entity_to_net_entity(&self, entity: &Entity) -> NetEntity {
        todo!()
    }

    fn net_entity_to_entity(&self, net_entity: &NetEntity) -> Entity {
        todo!()
    }
}
