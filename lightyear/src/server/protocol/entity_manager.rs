use std::{
    clone::Clone,
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    time::Duration,
};
use bevy_ecs::component::ComponentId;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use crate::server::sequence_list::SequenceList;

use crate::shared::{
    serde::{BitWrite, BitWriter, Serde, UnsignedVariableInteger},
    wrapping_diff, ChannelId, DiffMask, EntityAction, EntityActionType,
    EntityConverter, Instant, Message, MessageIndex, MessageManager, NetEntity, NetEntityConverter,
    PacketIndex, PacketNotifiable,
};


use super::{
    entity_action_event::EntityActionEvent, world_channel::WorldChannel,
};

const DROP_UPDATE_RTT_FACTOR: f32 = 1.5;
const ACTION_RECORD_TTL: Duration = Duration::from_secs(60);

pub type ActionId = MessageIndex;

/// Manages Entities for a given Client connection and keeps them in
/// sync on the Client
pub struct EntityManager {
    // World
    world_channel: WorldChannel,
    next_send_actions: VecDeque<(ActionId, EntityActionEvent)>,
    #[allow(clippy::type_complexity)]
    sent_action_packets: SequenceList<(Instant, Vec<(ActionId, EntityAction)>)>,

    // Updates
    next_send_updates: HashMap<Entity, HashSet<ComponentId>>,
    #[allow(clippy::type_complexity)]
    /// Map of component updates and [`DiffMask`] that were written into each packet
    sent_updates: HashMap<PacketIndex, (Instant, HashMap<(Entity, ComponentId), DiffMask>)>,
    /// Last [`PacketIndex`] where a component update was written by the server
    last_update_packet_index: PacketIndex,
}

impl EntityManager {
    /// Create a new EntityManager, given the client's address
    pub fn new(address: SocketAddr) -> Self {
        EntityManager {
            // World
            world_channel: WorldChannel::new(address),
            next_send_actions: VecDeque::new(),
            sent_action_packets: SequenceList::new(),

            // Update
            next_send_updates: HashMap::new(),
            sent_updates: HashMap::new(),
            last_update_packet_index: 0,
        }
    }

    // World Scope

    pub fn spawn_entity(&mut self, entity: &Entity) {
        self.world_channel.host_spawn_entity(entity);
    }

    pub fn despawn_entity(&mut self, entity: &Entity) {
        self.world_channel.host_despawn_entity(entity);
    }

    pub fn insert_component(&mut self, entity: &Entity, component_id: &ComponentId) {
        self.world_channel
            .host_insert_component(entity, component_id);
    }

    pub fn remove_component(&mut self, entity: &Entity, component_id: &ComponentId) {
        self.world_channel
            .host_remove_component(entity, component_id);
    }

    pub fn scope_has_entity(&self, entity: &Entity) -> bool {
        self.world_channel.host_has_entity(entity)
    }

    pub fn entity_channel_is_open(&self, entity: &Entity) -> bool {
        self.world_channel.entity_channel_is_open(entity)
    }

    // Messages

    pub fn queue_entity_message(
        &mut self,
        entities: Vec<Entity>,
        channel: &ChannelId,
        message: Box<dyn Message>,
    ) {
        self.world_channel
            .delayed_entity_messages
            .queue_message(entities, channel, message);
    }

    // Writer

    pub fn collect_outgoing_messages(
        &mut self,
        now: &Instant,
        rtt_millis: &f32,
        message_manager: &mut MessageManager,
    ) {
        self.world_channel
            .delayed_entity_messages
            .collect_ready_messages(message_manager);

        self.collect_dropped_update_packets(rtt_millis);

        self.collect_dropped_action_packets();
        self.collect_next_actions(now, rtt_millis);

        self.collect_component_updates();
    }

    pub fn has_outgoing_messages(&self) -> bool {
        !self.next_send_actions.is_empty() || !self.next_send_updates.is_empty()
    }

    // Collecting

    fn collect_dropped_action_packets(&mut self) {
        let mut pop = false;

        loop {
            if let Some((_, (time_sent, _))) = self.sent_action_packets.front() {
                if time_sent.elapsed() > ACTION_RECORD_TTL {
                    pop = true;
                }
            } else {
                return;
            }
            if pop {
                self.sent_action_packets.pop_front();
            } else {
                return;
            }
        }
    }

    fn collect_next_actions(&mut self, now: &Instant, rtt_millis: &f32) {
        self.next_send_actions = self.world_channel.take_next_actions(now, rtt_millis);
    }

    fn collect_dropped_update_packets(&mut self, rtt_millis: &f32) {
        let drop_duration = Duration::from_millis((DROP_UPDATE_RTT_FACTOR * rtt_millis) as u64);

        {
            let mut dropped_packets = Vec::new();
            for (packet_index, (time_sent, _)) in &self.sent_updates {
                if time_sent.elapsed() > drop_duration {
                    dropped_packets.push(*packet_index);
                }
            }

            for packet_index in dropped_packets {
                self.dropped_update_cleanup(packet_index);
            }
        }
    }

    fn dropped_update_cleanup(&mut self, dropped_packet_index: PacketIndex) {
        if let Some((_, diff_mask_map)) = self.sent_updates.remove(&dropped_packet_index) {
            for (component_index, diff_mask) in &diff_mask_map {
                let (entity, component) = component_index;
                if !self
                    .world_channel
                    .diff_handler
                    .has_component(entity, component)
                {
                    continue;
                }
                let mut new_diff_mask = diff_mask.clone();

                // walk from dropped packet up to most recently sent packet
                if dropped_packet_index == self.last_update_packet_index {
                    continue;
                }

                let mut packet_index = dropped_packet_index.wrapping_add(1);
                while packet_index != self.last_update_packet_index {
                    if let Some((_, diff_mask_map)) = self.sent_updates.get(&packet_index) {
                        if let Some(next_diff_mask) = diff_mask_map.get(component_index) {
                            new_diff_mask.nand(next_diff_mask);
                        }
                    }

                    packet_index = packet_index.wrapping_add(1);
                }

                self.world_channel
                    .diff_handler
                    .or_diff_mask(entity, component, &new_diff_mask);
            }
        }
    }

    fn collect_component_updates(&mut self) {
        self.next_send_updates = self.world_channel.collect_next_updates();
    }

    // Writing actions

    fn write_action_id(
        bit_writer: &mut dyn BitWrite,
        last_id_opt: &mut Option<ActionId>,
        current_id: &ActionId,
    ) {
        if let Some(last_id) = last_id_opt {
            // write diff
            let id_diff = wrapping_diff(*last_id, *current_id);
            let id_diff_encoded = UnsignedVariableInteger::<3>::new(id_diff);
            id_diff_encoded.ser(bit_writer);
        } else {
            // write message id
            current_id.ser(bit_writer);
        }
        *last_id_opt = Some(*current_id);
    }

    pub fn write_actions(
        &mut self,
        now: &Instant,
        writer: &mut BitWriter,
        packet_index: &PacketIndex,
        world: &World,
        has_written: &mut bool,
    ) {
        let mut last_counted_id: Option<MessageIndex> = None;
        let mut last_written_id: Option<MessageIndex> = None;

        loop {
            if self.next_send_actions.is_empty() {
                break;
            }

            // check that we can write the next message
            let mut counter = writer.counter();
            self.write_action(
                world,
                packet_index,
                &mut counter,
                &mut last_counted_id,
                false,
            );

            if counter.overflowed() {
                // if nothing useful has been written in this packet yet,
                // send warning about size of component being too big
                if !*has_written {
                    self.warn_overflow_action(
                        world,
                        counter.bits_needed(),
                        writer.bits_free(),
                    );
                }
                break;
            }

            *has_written = true;

            // write ActionContinue bit
            true.ser(writer);

            // optimization
            if !self
                .sent_action_packets
                .contains_scan_from_back(packet_index)
            {
                self.sent_action_packets
                    .insert_scan_from_back(*packet_index, (now.clone(), Vec::new()));
            }

            // write data
            self.write_action(
                world,
                packet_index,
                writer,
                &mut last_written_id,
                true,
            );

            // pop action we've written
            self.next_send_actions.pop_front();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_action(
        &mut self,
        world: &World,
        packet_index: &PacketIndex,
        bit_writer: &mut dyn BitWrite,
        last_written_id: &mut Option<ActionId>,
        is_writing: bool,
    ) {
        let (action_id, action) = self.next_send_actions.front().unwrap();

        // write message id
        Self::write_action_id(bit_writer, last_written_id, action_id);


        match action {
            EntityActionEvent::SpawnEntity(entity) => {
                EntityActionType::SpawnEntity.ser(bit_writer);

                // write net entity
                self.world_channel
                    .entity_to_net_entity(entity)
                    .unwrap()
                    .ser(bit_writer);

                // get component list
                let component_kinds: Vec<ComponentId> = self.get_component_ids(world, *entity).collect();

                // write number of components
                let components_num =
                    UnsignedVariableInteger::<3>::new(component_kinds.len() as i128);
                components_num.ser(bit_writer);

                for component_kind in &component_kinds {
                    let converter = EntityConverter::new(self);

                    // write component payload
                    world
                        .component_of_kind(entity, component_kind)
                        .expect("Component does not exist in World")
                        .write(bit_writer, &converter);
                }

                // if we are writing to this packet, add it to record
                if is_writing {
                    //info!("write SpawnEntity({})", action_id);

                    Self::record_action_written(
                        &mut self.sent_action_packets,
                        packet_index,
                        action_id,
                        EntityAction::SpawnEntity(*entity, component_kinds),
                    );
                }
            }
            EntityActionEvent::DespawnEntity(entity) => {
                EntityActionType::DespawnEntity.ser(bit_writer);

                // write net entity
                self.world_channel
                    .entity_to_net_entity(entity)
                    .unwrap()
                    .ser(bit_writer);

                // if we are writing to this packet, add it to record
                if is_writing {
                    //info!("write DespawnEntity({})", action_id);

                    Self::record_action_written(
                        &mut self.sent_action_packets,
                        packet_index,
                        action_id,
                        EntityAction::DespawnEntity(*entity),
                    );
                }
            }
            EntityActionEvent::InsertComponent(entity, component) => {
                if !world.has_component_of_kind(entity, component)
                    || !self.world_channel.entity_channel_is_open(entity)
                {
                    EntityActionType::Noop.ser(bit_writer);

                    // if we are actually writing this packet
                    if is_writing {
                        //info!("write Noop({})", action_id);

                        // add it to action record
                        Self::record_action_written(
                            &mut self.sent_action_packets,
                            packet_index,
                            action_id,
                            EntityAction::Noop,
                        );
                    }
                } else {
                    EntityActionType::InsertComponent.ser(bit_writer);

                    // write net entity
                    self.world_channel
                        .entity_to_net_entity(entity)
                        .unwrap()
                        .ser(bit_writer);

                    let converter = EntityConverter::new(self);

                    // write component payload
                    world
                        .component_of_kind(entity, component)
                        .expect("Component does not exist in World")
                        .write(bit_writer, &converter);

                    // if we are actually writing this packet
                    if is_writing {
                        //info!("write InsertComponent({})", action_id);

                        // add it to action record
                        Self::record_action_written(
                            &mut self.sent_action_packets,
                            packet_index,
                            action_id,
                            EntityAction::InsertComponent(*entity, *component),
                        );
                    }
                }
            }
            EntityActionEvent::RemoveComponent(entity, component) => {
                if !self.world_channel.entity_channel_is_open(entity) {
                    EntityActionType::Noop.ser(bit_writer);

                    // if we are actually writing this packet
                    if is_writing {
                        //info!("write Noop({})", action_id);

                        // add it to action record
                        Self::record_action_written(
                            &mut self.sent_action_packets,
                            packet_index,
                            action_id,
                            EntityAction::Noop,
                        );
                    }
                } else {
                    EntityActionType::RemoveComponent.ser(bit_writer);

                    // write net entity
                    self.world_channel
                        .entity_to_net_entity(entity)
                        .unwrap()
                        .ser(bit_writer);

                    // write component kind
                    component.ser(bit_writer);

                    // if we are writing to this packet, add it to record
                    if is_writing {
                        //info!("write RemoveComponent({})", action_id);

                        Self::record_action_written(
                            &mut self.sent_action_packets,
                            packet_index,
                            action_id,
                            EntityAction::RemoveComponent(*entity, *component),
                        );
                    }
                }
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn record_action_written(
        sent_actions: &mut SequenceList<(Instant, Vec<(ActionId, EntityAction)>)>,
        packet_index: &PacketIndex,
        action_id: &ActionId,
        action_record: EntityAction,
    ) {
        let (_, sent_actions_list) = sent_actions.get_mut_scan_from_back(packet_index).unwrap();
        sent_actions_list.push((*action_id, action_record));
    }

    fn warn_overflow_action(
        &self,
        world: &World,
        bits_needed: u16,
        bits_free: u16,
    ) {
        let (_action_id, action) = self.next_send_actions.front().unwrap();

        match action {
            EntityActionEvent::SpawnEntity(entity) => {
                let mut component_names = "".to_owned();
                let mut added = false;

                for component_kind in &self.get_component_ids(world, *entity) {
                    if added {
                        component_names.push(',');
                    } else {
                        added = true;
                    }
                    let name = component_kind.name();
                    component_names.push_str(&name);
                }
                panic!(
                    "Packet Write Error: Blocking overflow detected! Entity Spawn message with Components `{component_names}` requires {bits_needed} bits, but packet only has {bits_free} bits available! Recommend slimming down these Components."
                )
            }
            EntityActionEvent::InsertComponent(_entity, component) => {
                let component_name = component.name();
                panic!(
                    "Packet Write Error: Blocking overflow detected! Component Insertion message of type `{component_name}` requires {bits_needed} bits, but packet only has {bits_free} bits available! This condition should never be reached, as large Messages should be Fragmented in the Reliable channel"
                )
            }
            _ => {
                panic!(
                    "Packet Write Error: Blocking overflow detected! Action requires {bits_needed} bits, but packet only has {bits_free} bits available! This message should never display..."
                )
            }
        }
    }

    pub fn write_updates(
        &mut self,
        now: &Instant,
        writer: &mut BitWriter,
        packet_index: &PacketIndex,
        world: &World,
        has_written: &mut bool,
    ) {
        let all_update_entities: Vec<Entity> = self.next_send_updates.keys().copied().collect();

        for entity in all_update_entities {
            // check that we can at least write a NetEntityId and a ComponentContinue bit
            let mut counter = writer.counter();

            let net_entity_id = self.world_channel.entity_to_net_entity(&entity).unwrap();
            net_entity_id.ser(&mut counter);

            counter.write_bit(false);

            if counter.overflowed() {
                break;
            }

            // write UpdateContinue bit
            true.ser(writer);

            // reserve ComponentContinue bit
            writer.reserve_bits(1);

            // write NetEntityId
            net_entity_id.ser(writer);

            // write Components
            self.write_update(
                now,
                world,
                packet_index,
                writer,
                &entity,
                has_written,
            );

            // write ComponentContinue finish bit, release
            false.ser(writer);
            writer.release_bits(1);
        }
    }

    /// For a given entity, write component value updates into a packet
    /// Only component values that changed in the internal (naia's) host world will be written
    fn write_update(
        &mut self,
        now: &Instant,
        world: &World,
        packet_index: &PacketIndex,
        writer: &mut BitWriter,
        entity: &Entity,
        has_written: &mut bool,
    ) {
        let mut written_component_kinds = Vec::new();
        let component_kinds = self.next_send_updates.get(entity).unwrap();
        for component_kind in component_kinds {
            // get diff mask
            let diff_mask = self
                .world_channel
                .diff_handler
                .diff_mask(entity, component_kind)
                .expect("DiffHandler does not have registered Component!")
                .clone();

            let converter = EntityConverter::new(self);

            // check that we can write the next component update
            let mut counter = writer.counter();
            component_kind.ser(&mut counter);
            world
                .component_of_kind(entity, component_kind)
                .expect("Component does not exist in World")
                .write_update(&diff_mask, &mut counter, &converter);

            if counter.overflowed() {
                // if nothing useful has been written in this packet yet,
                // send warning about size of component being too big
                if !*has_written {
                    self.warn_overflow_update(
                        component_kind,
                        counter.bits_needed(),
                        writer.bits_free(),
                    );
                }

                break;
            }

            *has_written = true;

            // write ComponentContinue bit
            true.ser(writer);

            // write component kind
            component_kind.ser(writer);

            // write data
            world
                .component_of_kind(entity, component_kind)
                .expect("Component does not exist in World")
                .write_update(&diff_mask, writer, &converter);

            written_component_kinds.push(*component_kind);

            //info!("writing UpdateComponent");

            // place diff mask in a special transmission record - like map
            self.last_update_packet_index = *packet_index;

            if !self.sent_updates.contains_key(packet_index) {
                self.sent_updates
                    .insert(*packet_index, (now.clone(), HashMap::new()));
            }
            let (_, sent_updates_map) = self.sent_updates.get_mut(packet_index).unwrap();
            sent_updates_map.insert((*entity, *component_kind), diff_mask);

            // having copied the diff mask for this update, clear the component
            self.world_channel
                .diff_handler
                .clear_diff_mask(entity, component_kind);
        }

        let update_kinds = self.next_send_updates.get_mut(entity).unwrap();
        for component_kind in &written_component_kinds {
            update_kinds.remove(component_kind);
        }
        if update_kinds.is_empty() {
            self.next_send_updates.remove(entity);
        }
    }

    fn warn_overflow_update(&self, component_kind: &ComponentId, bits_needed: u16, bits_free: u16) {
        let component_name = component_kind.name();
        panic!(
            "Packet Write Error: Blocking overflow detected! Data update of Component `{component_name}` requires {bits_needed} bits, but packet only has {bits_free} bits available! Recommended to slim down this Component"
        )
    }

    /// Get an iterator of all component ids attached to this entity
    fn get_component_ids(&self, world: &World, entity: Entity) -> impl Iterator<Item=ComponentId> + '_ {
        world.entity(entity).archetype().components()
    }
}

// PacketNotifiable
impl PacketNotifiable for EntityManager {
    fn notify_packet_delivered(&mut self, packet_index: PacketIndex) {
        // Updates
        self.sent_updates.remove(&packet_index);

        // Actions
        if let Some((_, action_list)) = self
            .sent_action_packets
            .remove_scan_from_front(&packet_index)
        {
            for (action_id, action) in action_list {
                self.world_channel.action_delivered(action_id, action);
            }
        }
    }
}

// NetEntityConverter
impl NetEntityConverter for EntityManager {
    fn entity_to_net_entity(&self, entity: &Entity) -> NetEntity {
        return *self
            .world_channel
            .entity_to_net_entity(entity)
            .expect("entity does not exist for this connection!");
    }

    fn net_entity_to_entity(&self, net_entity: &NetEntity) -> Entity {
        return *self
            .world_channel
            .net_entity_to_entity(net_entity)
            .expect("entity does not exist for this connection!");
    }
}
