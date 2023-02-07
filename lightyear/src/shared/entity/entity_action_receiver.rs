use std::{
    collections::{HashMap, VecDeque},
};
use bevy_ecs::component::ComponentId;
use bevy_ecs::entity::Entity;

use crate::shared::{EntityAction, NetEntity, sequence_less_than, UnorderedReliableReceiver, MessageId as ActionId};

pub struct EntityActionReceiver {
    receiver: UnorderedReliableReceiver<EntityAction>,
    entity_channels: HashMap<NetEntity, EntityChannel>,
}

impl Default for EntityActionReceiver {
    fn default() -> Self {
        Self {
            receiver: UnorderedReliableReceiver::default(),
            entity_channels: HashMap::default(),
        }
    }
}

impl EntityActionReceiver {
    /// Buffer a read [`EntityAction`] so that it can be processed later
    pub fn buffer_action(&mut self, action_id: ActionId, action: EntityAction) {
        self.receiver.buffer_message(action_id, action)
    }

    /// Read all buffered [`EntityAction`] inside the `receiver` and process them.
    ///
    /// Outputs the list of [`EntityAction`] that can be executed now, buffer the rest
    /// into each entity's [`EntityChannel`]
    pub fn receive_actions(&mut self) -> Vec<EntityAction> {
        let mut outgoing_actions = Vec::new();
        let incoming_actions = self.receiver.receive_messages();
        for (action_id, action) in incoming_actions {
            if let Some(entity) = action.entity() {
                self.entity_channels
                    .entry(entity)
                    .or_insert_with(|| EntityChannel::new(entity));
                let entity_channel = self.entity_channels.get_mut(&entity).unwrap();
                entity_channel.receive_action(action_id, action, &mut outgoing_actions);
            }
        }
        outgoing_actions
    }
}

// Entity Channel

pub struct EntityChannel {
    entity: Entity,
    last_canonical_id: Option<ActionId>,
    spawned: bool,
    components: HashMap<ComponentId, ComponentChannel>,
    waiting_spawns: OrderedIds<Vec<ComponentId>>,
    waiting_despawns: OrderedIds<()>,
}

impl EntityChannel {
    pub fn new(entity: Entity) -> Self {
        Self {
            entity,
            spawned: false,
            components: HashMap::new(),
            waiting_spawns: OrderedIds::new(),
            waiting_despawns: OrderedIds::new(),
            last_canonical_id: None,
        }
    }

    /// Process the provided [`EntityAction`]:
    ///
    /// * Checks that [`EntityAction`] can be executed now
    /// * If so, add it to `outgoing_actions`
    /// * Else, add it to internal "waiting" buffers so we can check when the [`EntityAction`]
    ///   can be executed
    ///
    /// ([`EntityAction`]s might not be executable now, for example is an InsertComponent
    ///  is processed before the corresponding entity has been spawned)
    pub fn receive_action(
        &mut self,
        incoming_action_id: ActionId,
        incoming_action: EntityAction,
        outgoing_actions: &mut Vec<EntityAction>,
    ) {
        match incoming_action {
            EntityAction::SpawnEntity(_, components) => {
                self.receive_spawn_entity_action(incoming_action_id, components, outgoing_actions);
            }
            EntityAction::DespawnEntity(_) => {
                self.receive_despawn_entity_action(incoming_action_id, outgoing_actions);
            }
            EntityAction::InsertComponent(_, component) => {
                self.receive_insert_component_action(
                    incoming_action_id,
                    component,
                    outgoing_actions,
                );
            }
            EntityAction::RemoveComponent(_, component) => {
                self.receive_remove_component_action(
                    incoming_action_id,
                    component,
                    outgoing_actions,
                );
            }
            EntityAction::Noop => {}
        }
    }

    /// Process the entity action.
    /// When the entity is actually spawned on the client, send back an ack event
    /// to the server.
    pub fn receive_spawn_entity_action(
        &mut self,
        id: ActionId,
        components: Vec<ComponentId>,
        outgoing_actions: &mut Vec<EntityAction>,
    ) {
        // do not process any spawn OLDER than last received spawn id / despawn id
        if let Some(last_id) = self.last_canonical_id {
            if sequence_less_than(id, last_id) {
                return;
            }
        }

        if !self.spawned {
            self.spawned = true;
            outgoing_actions.push(EntityAction::SpawnEntity(self.entity, components));

            // pop ALL waiting spawns, despawns, inserts, and removes OLDER than spawn_id
            self.receive_canonical(id);

            // process any waiting despawns
            if let Some((despawn_id, _)) = self.waiting_despawns.inner.pop_front() {
                self.receive_despawn_entity_action(despawn_id, outgoing_actions);
            } else {
                // process any waiting inserts
                let mut inserted_components = Vec::new();
                for (component, component_state) in &mut self.components {
                    if let Some(insert_id) = component_state.waiting_inserts.inner.pop_front() {
                        inserted_components.push((insert_id, *component));
                    }
                }

                for ((id, _), component) in inserted_components {
                    self.receive_insert_component_action(id, component, outgoing_actions);
                }
            }
        } else {
            // buffer spawn for later
            self.waiting_spawns.push_back(id, components);
        }
    }

    /// Process the entity despawn action
    /// When the entity has actually been despawned on the client, add an ack to the
    /// `outgoing_actions`
    pub fn receive_despawn_entity_action(
        &mut self,
        id: ActionId,
        outgoing_actions: &mut Vec<EntityAction>,
    ) {
        // do not process any despawn OLDER than last received spawn id / despawn id
        if let Some(last_id) = self.last_canonical_id {
            if sequence_less_than(id, last_id) {
                return;
            }
        }

        if self.spawned {
            self.spawned = false;
            outgoing_actions.push(EntityAction::DespawnEntity(self.entity));

            // pop ALL waiting spawns, despawns, inserts, and removes OLDER than despawn_id
            self.receive_canonical(id);

            // set all component channels to 'inserted = false'
            for value in self.components.values_mut() {
                value.inserted = false;
            }

            // process any waiting spawns
            if let Some((spawn_id, components)) = self.waiting_spawns.inner.pop_front() {
                self.receive_spawn_entity_action(spawn_id, components, outgoing_actions);
            }
        } else {
            // buffer despawn for later
            self.waiting_despawns.push_back(id, ());
        }
    }

    pub fn receive_insert_component_action(
        &mut self,
        id: ActionId,
        component: ComponentId,
        outgoing_actions: &mut Vec<EntityAction>,
    ) {
        // do not process any insert OLDER than last received spawn id / despawn id
        if let Some(last_id) = self.last_canonical_id {
            if sequence_less_than(id, last_id) {
                return;
            }
        }

        if let std::collections::hash_map::Entry::Vacant(e) = self.components.entry(component) {
            e.insert(ComponentChannel::new(self.last_canonical_id));
        }
        let component_state = self.components.get_mut(&component).unwrap();

        // do not process any insert OLDER than last received insert / remove id for
        // this component
        if let Some(last_id) = component_state.last_canonical_id {
            if sequence_less_than(id, last_id) {
                return;
            }
        }

        if !component_state.inserted {
            component_state.inserted = true;
            outgoing_actions.push(EntityAction::InsertComponent(self.entity, component));

            // pop ALL waiting inserts, and removes OLDER than insert_id (in reference to
            // component)
            component_state.receive_canonical(id);

            // process any waiting removes
            if let Some((remove_id, _)) = component_state.waiting_removes.inner.pop_front() {
                self.receive_remove_component_action(remove_id, component, outgoing_actions);
            }
        } else {
            // buffer insert
            component_state.waiting_inserts.push_back(id, ());
        }
    }

    pub fn receive_remove_component_action(
        &mut self,
        id: ActionId,
        component: ComponentId,
        outgoing_actions: &mut Vec<EntityAction>,
    ) {
        // do not process any remove OLDER than last received spawn id / despawn id
        if let Some(last_id) = self.last_canonical_id {
            if sequence_less_than(id, last_id) {
                return;
            }
        }

        if let std::collections::hash_map::Entry::Vacant(e) = self.components.entry(component) {
            e.insert(ComponentChannel::new(self.last_canonical_id));
        }
        let component_state = self.components.get_mut(&component).unwrap();

        // do not process any remove OLDER than last received insert / remove id for
        // this component
        if let Some(last_id) = component_state.last_canonical_id {
            if sequence_less_than(id, last_id) {
                return;
            }
        }

        if component_state.inserted {
            component_state.inserted = false;
            outgoing_actions.push(EntityAction::RemoveComponent(self.entity, component));

            // pop ALL waiting inserts, and removes OLDER than remove_id (in reference to
            // component)
            component_state.receive_canonical(id);

            // process any waiting inserts
            if let Some((insert_id, _)) = component_state.waiting_inserts.inner.pop_front() {
                self.receive_insert_component_action(insert_id, component, outgoing_actions);
            }
        } else {
            // buffer remove
            component_state.waiting_removes.push_back(id, ());
        }
    }

    pub fn receive_canonical(&mut self, id: ActionId) {
        // pop ALL waiting spawns, despawns, inserts, and removes OLDER than id
        self.waiting_spawns.pop_front_until_and_including(id);
        self.waiting_despawns.pop_front_until_and_including(id);
        for component_state in self.components.values_mut() {
            component_state.receive_canonical(id);
        }

        self.last_canonical_id = Some(id);
    }
}

// Component Channel
// most of this should be public, no methods here

pub struct ComponentChannel {
    pub inserted: bool,
    pub last_canonical_id: Option<ActionId>,
    pub waiting_inserts: OrderedIds<()>,
    pub waiting_removes: OrderedIds<()>,
}

impl ComponentChannel {
    pub fn new(canonical_id: Option<ActionId>) -> Self {
        Self {
            inserted: false,
            waiting_inserts: OrderedIds::new(),
            waiting_removes: OrderedIds::new(),
            last_canonical_id: canonical_id,
        }
    }

    pub fn receive_canonical(&mut self, id: ActionId) {
        // pop ALL waiting inserts, and removes OLDER than id
        self.waiting_inserts.pop_front_until_and_including(id);
        self.waiting_removes.pop_front_until_and_including(id);

        self.last_canonical_id = Some(id);
    }
}

pub struct OrderedIds<P> {
    // front small, back big
    inner: VecDeque<(ActionId, P)>,
}

impl<P> OrderedIds<P> {
    pub fn new() -> Self {
        Self {
            inner: VecDeque::new(),
        }
    }

    // pub fn push_front(&mut self, id: ActionId) {
    //     let mut index = 0;
    //
    //     loop {
    //         if index == self.inner.len() {
    //             self.inner.push_back(id);
    //             return;
    //         }
    //
    //         let old_id = self.inner.get(index).unwrap();
    //         if sequence_greater_than(*old_id, id) {
    //             self.inner.insert(index, id);
    //             return;
    //         }
    //
    //         index += 1
    //     }
    // }

    pub fn push_back(&mut self, id: ActionId, item: P) {
        let mut index = self.inner.len();

        loop {
            if index == 0 {
                self.inner.push_front((id, item));
                return;
            }

            index -= 1;

            let (old_id, _) = self.inner.get(index).unwrap();
            if sequence_less_than(*old_id, id) {
                self.inner.insert(index + 1, (id, item));
                return;
            }
        }
    }

    pub fn pop_front_until_and_including(&mut self, id: ActionId) {
        let mut pop = false;

        if let Some((old_id, _)) = self.inner.front() {
            if *old_id == id || sequence_less_than(*old_id, id) {
                pop = true;
            }
        }

        if pop {
            self.inner.pop_front();
        }
    }
}
