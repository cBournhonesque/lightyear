use crate::serialize::reader::{ReadVarInt, Reader};
use crate::serialize::varint::varint_len;
use crate::serialize::writer::WriteInteger;
use crate::serialize::{SerializationError, ToBytes};
use bevy::platform::collections::HashMap;
use core::any::TypeId;
use core::hash::Hash;

/// ID used to serialize IDs over the network efficiently
pub(crate) type NetId = u16;

impl ToBytes for NetId {
    fn bytes_len(&self) -> usize {
        varint_len(*self as u64)
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
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

    #[cfg(test)]
    pub(in crate::protocol) fn len(&self) -> usize {
        self.kind_map.len()
    }
}
