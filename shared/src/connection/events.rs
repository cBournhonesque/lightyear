use std::collections::HashMap;

use bevy_ecs::prelude::Entity;

use crate::{ChannelKind, Protocol};

// TODO: don't make fields pub but instead make accessors
pub struct Events<P: Protocol> {
    // netcode
    // connections: Vec<ClientId>,
    // disconnections: Vec<ClientId>,

    // messages
    pub messages: HashMap<ChannelKind, Vec<P::Message>>,
    // replication
    pub spawns: Vec<Entity>,
    pub despawns: Vec<Entity>,
    // TODO: key by entity or by kind?
    pub insert_components: HashMap<Entity, Vec<P::Components>>,
    pub remove_components: HashMap<Entity, Vec<P::ComponentKinds>>,
    pub update_components: HashMap<Entity, Vec<P::Components>>,
    empty: bool,
}

impl<P: Protocol> Default for Events<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Protocol> Events<P> {
    pub fn new() -> Self {
        Self {
            // netcode
            // connections: Vec::new(),
            // disconnections: Vec::new(),
            // messages
            messages: HashMap::new(),
            // replication
            spawns: Vec::new(),
            despawns: Vec::new(),
            insert_components: Default::default(),
            remove_components: Default::default(),
            update_components: Default::default(),
            // bookkeeping
            empty: true,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.empty
    }
    pub(crate) fn push_message(&mut self, channel_kind: ChannelKind, message: P::Message) {
        self.messages.entry(channel_kind).or_default().push(message);
        self.empty = false;
    }

    pub(crate) fn push_spawn(&mut self, entity: Entity) {
        self.spawns.push(entity);
        self.empty = false;
    }

    pub(crate) fn push_insert_component(&mut self, entity: Entity, component: P::Components) {
        self.insert_components
            .entry(entity)
            .or_default()
            .push(component);
        self.empty = false;
    }
}
