use std::{
    collections::HashMap,
    hash::Hash,
    net::SocketAddr,
    sync::{Arc, RwLock, RwLockReadGuard},
};

use crate::shared::{ComponentId, DiffMask};

use super::{global_diff_handler::GlobalDiffHandler, mut_channel::MutReceiver};

#[derive(Clone)]
pub struct UserDiffHandler<E: Copy + Eq + Hash> {
    receivers: HashMap<(E, ComponentId), MutReceiver>,
    global_diff_handler: Arc<RwLock<GlobalDiffHandler<E>>>,
}

impl<E: Copy + Eq + Hash> UserDiffHandler<E> {
    pub fn new(global_diff_handler: &Arc<RwLock<GlobalDiffHandler<E>>>) -> Self {
        UserDiffHandler {
            receivers: HashMap::new(),
            global_diff_handler: global_diff_handler.clone(),
        }
    }

    // Component Registration
    pub fn register_component(
        &mut self,
        addr: &SocketAddr,
        entity: &E,
        component_kind: &ComponentId,
    ) {
        if let Ok(global_handler) = self.global_diff_handler.as_ref().read() {
            let receiver = global_handler
                .receiver(addr, entity, component_kind)
                .expect("GlobalDiffHandler has not yet registered this Component");
            self.receivers.insert((*entity, *component_kind), receiver);
        }
    }

    pub fn deregister_component(&mut self, entity: &E, component_kind: &ComponentId) {
        self.receivers.remove(&(*entity, *component_kind));
    }

    pub fn has_component(&self, entity: &E, component: &ComponentId) -> bool {
        self.receivers.contains_key(&(*entity, *component))
    }

    // Diff masks
    pub fn diff_mask(
        &self,
        entity: &E,
        component_kind: &ComponentId,
    ) -> Option<RwLockReadGuard<DiffMask>> {
        if let Some(receiver) = self.receivers.get(&(*entity, *component_kind)) {
            return receiver.mask();
        }
        None
    }

    //    pub fn has_diff_mask(&self, component_key: &ComponentKey) -> bool {
    //        return self.receivers.contains_key(component_key);
    //    }

    pub fn diff_mask_is_clear(&self, entity: &E, component_kind: &ComponentId) -> Option<bool> {
        if let Some(receiver) = self.receivers.get(&(*entity, *component_kind)) {
            return Some(receiver.diff_mask_is_clear());
        }
        None
    }

    pub fn or_diff_mask(
        &mut self,
        entity: &E,
        component_kind: &ComponentId,
        other_mask: &DiffMask,
    ) {
        let current_diff_mask = self.receivers.get_mut(&(*entity, *component_kind)).unwrap();
        current_diff_mask.or_mask(other_mask);
    }

    pub fn clear_diff_mask(&mut self, entity: &E, component_kind: &ComponentId) {
        let receiver = self.receivers.get_mut(&(*entity, *component_kind)).unwrap();
        receiver.clear_mask();
    }
}
