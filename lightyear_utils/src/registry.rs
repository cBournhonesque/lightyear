use crate::collections::HashMap;
use bevy::platform::hash::{DefaultHasher, FixedHasher};
use core::any::TypeId;
use core::fmt::Formatter;
use core::hash::{BuildHasher, Hash, Hasher};

/// ID used to serialize IDs over the network efficiently
pub(crate) type NetId = u16;

pub trait TypeKind: From<TypeId> + Copy + PartialEq + Eq + Hash {}

/// Struct to map a type to an id that can be serialized over the network
#[derive(Clone, Debug, PartialEq)]
pub struct TypeMapper<K: TypeKind> {
    pub(crate) next_net_id: NetId,
    pub(crate) kind_map: HashMap<K, NetId>,
    pub(crate) id_map: HashMap<NetId, K>,
}

impl<K: TypeKind> Default for TypeMapper<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: TypeKind> TypeMapper<K> {
    pub fn new() -> Self {
        Self {
            next_net_id: 0,
            kind_map: HashMap::default(),
            id_map: HashMap::default(),
        }
    }

    /// Register a new type
    pub fn add<T: 'static>(&mut self) -> K {
        let kind = K::from(TypeId::of::<T>());
        if self.kind_map.contains_key(&kind) {
            panic!("Type {:?} already registered", core::any::type_name::<T>());
        }
        let net_id = self.next_net_id;
        self.kind_map.insert(kind, net_id);
        self.id_map.insert(net_id, kind);
        self.next_net_id += 1;
        kind
    }

    pub fn kind(&self, net_id: NetId) -> Option<&K> {
        self.id_map.get(&net_id)
    }

    pub fn net_id(&self, kind: &K) -> Option<&NetId> {
        self.kind_map.get(kind)
    }
}

#[derive(Clone)]
pub struct RegistryHasher{
    hasher: DefaultHasher,
    hash: Option<RegistryHash>
}

pub type RegistryHash = u64;

impl core::fmt::Debug for RegistryHasher {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "RegistryHasher")
    }
}

impl Default for RegistryHasher {
    fn default() -> Self {
        Self{
           hasher: FixedHasher.build_hasher(),
           hash: None,
        }
    }
}

impl RegistryHasher {
    pub fn hash<T>(&mut self) {
        if self.hash.is_some() {
            panic!("Tried to register type {:?} after the protocol was finished", core::any::type_name::<T>())
        }
        core::any::type_name::<T>().hash(&mut self.hasher);
    }
    pub fn finish(&mut self) -> RegistryHash {
        match self.hash {
            None => {
                let hash = self.hasher.finish();
                self.hash = Some(hash);
                hash
            }
            Some(h) => h
        }
    }
}
