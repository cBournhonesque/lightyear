mod channel;
mod message;

use anyhow::bail;
use std::any::{Any, TypeId};
use std::collections::HashMap;

type NetId = u16;

// TODO: read https://willcrichton.net/rust-api-type-patterns/registries.html more in detail

trait TypeBuilder {}

/// Registry to allow the user to register custom types.
pub struct TypeRegistry {
    next_net_id: NetId,
    kind_map: HashMap<TypeId, (NetId, Box<dyn Any>)>,
    id_map: HashMap<NetId, TypeId>,
}

impl TypeRegistry {
    pub fn new() -> Self {
        Self {
            next_net_id: 0,
            kind_map: HashMap::new(),
            id_map: HashMap::new(),
        }
    }

    /// Register a new type
    pub fn add<T: Any + 'static>(&mut self, t: T) -> anyhow::Result<()> {
        let type_id = TypeId::of::<T>();
        if self.kind_map.contains_key(&type_id) {
            bail!("Type already registered");
        }
        let net_id = self.next_net_id;
        self.kind_map.insert(type_id, (net_id, Box::new(t)));
        self.id_map.insert(net_id, type_id);
        self.next_net_id += 1;
        Ok(())
    }

    /// Get the registered object for a given type
    pub fn get<T: Any + 'static>(&self) -> Option<&T> {
        let type_id = TypeId::of::<T>();
        self.kind_map
            .get(&type_id)
            .and_then(|(_, t)| t.downcast_ref())
    }
}
