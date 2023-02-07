use std::{collections::HashMap, hash::Hash, net::SocketAddr};

use crate::shared::ComponentId;

use super::mut_channel::{MutChannel, MutReceiver, MutReceiverBuilder, MutSender};

pub struct GlobalDiffHandler<E: Copy + Eq + Hash> {
    mut_receiver_builders: HashMap<(E, ComponentId), MutReceiverBuilder>,
}

impl<E: Copy + Eq + Hash> Default for GlobalDiffHandler<E> {
    fn default() -> Self {
        Self {
            mut_receiver_builders: HashMap::default(),
        }
    }
}

impl<E: Copy + Eq + Hash> GlobalDiffHandler<E> {
    // For Server
    pub fn register_component(
        &mut self,
        entity: &E,
        component_id: &ComponentId,
        diff_mask_length: u8,
    ) -> MutSender {
        if self
            .mut_receiver_builders
            .contains_key(&(*entity, *component_id))
        {
            panic!("Component cannot register with Server more than once!");
        }

        let (sender, builder) = MutChannel::new_channel(diff_mask_length);

        self.mut_receiver_builders
            .insert((*entity, *component_id), builder);

        sender
    }

    pub fn deregister_component(&mut self, entity: &E, component_id: &ComponentId) {
        self.mut_receiver_builders.remove(&(*entity, *component_id));
    }

    pub fn receiver(
        &self,
        addr: &SocketAddr,
        entity: &E,
        component_id: &ComponentId,
    ) -> Option<MutReceiver> {
        if let Some(builder) = self.mut_receiver_builders.get(&(*entity, *component_id)) {
            return builder.build(addr);
        }
        None
    }
}
