use std::{any::TypeId, hash::Hash};

use bevy::utils::HashMap;
use byteorder::WriteBytesExt;

use crate::serialize::{
    reader::Reader,
    varint::{varint_len, VarIntReadExt, VarIntWriteExt},
    SerializationError, ToBytes,
};

/// ID used to serialize IDs over the network efficiently
pub(crate) type NetId = u16;

impl ToBytes for NetId {
    fn len(&self) -> usize {
        varint_len(*self as u64)
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        buffer.write_varint(*self as u64)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(buffer.read_varint()? as NetId)
    }
}

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
            kind_map: HashMap::new(),
            id_map: HashMap::new(),
        }
    }

    /// Register a new type
    pub fn add<T: 'static>(&mut self) -> K {
        let kind = K::from(TypeId::of::<T>());
        if self.kind_map.contains_key(&kind) {
            panic!("Type {:?} already registered", std::any::type_name::<T>());
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

    #[cfg(test)]
    pub(in crate::protocol) fn len(&self) -> usize {
        self.kind_map.len()
    }
}
