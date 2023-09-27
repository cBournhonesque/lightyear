mod channel;
mod message;

use crate::channel::channel::ChannelBuilder;
use std::any::Any;

pub(crate) type NetId = u16;

// TODO: read https://willcrichton.net/rust-api-type-patterns/registries.html more in detail

trait TypeBuilder {}

/// Trait for types that can create their own builders
pub(crate) trait GetBuilder<T> {
    fn get_builder(&self) -> Box<T>;
}

#[macro_export]
macro_rules! type_registry {
    ($name: ident, $T:tt, $builder:tt, $($v:ident: $t:ty),*) => {
        use crate::registry::NetId;
        use anyhow::bail;
        use std::any::TypeId;
        use std::collections::HashMap;

        pub struct $name {
            next_net_id: NetId,
            kind_map: HashMap<TypeId, (NetId, Box<dyn $builder>)>,
            id_map: HashMap<NetId, TypeId>,
        }

        impl $name {
            pub fn new() -> Self {
                Self {
                    next_net_id: 0,
                    kind_map: HashMap::new(),
                    id_map: HashMap::new(),
                }
            }

            /// Register a new type
            pub fn add<T: $T + 'static>(&mut self, $($v: $t),*) -> anyhow::Result<()> {
                let type_id = TypeId::of::<T>();
                if self.kind_map.contains_key(&type_id) {
                    bail!("Type already registered");
                }
                let net_id = self.next_net_id;
                self.kind_map.insert(type_id, (net_id, T::get_builder($($v,)*)));
                self.id_map.insert(net_id, type_id);
                self.next_net_id += 1;
                Ok(())
            }

            /// Get the registered object for a given type
            pub fn get_from_type(&self, type_id: &TypeId) -> Option<&Box<dyn $builder>> {
                self.kind_map.get(type_id).and_then(|(_, t)| Some(t))
            }

            pub fn get_from_net_id(&self, net_id: NetId) -> Option<&Box<dyn $builder>> {
                let type_id = self.id_map.get(&net_id)?;
                self.get_from_type(type_id)
            }

            #[cfg(test)]
            fn len(&self) -> usize {
                self.kind_map.len()
            }
        }
    };
}
