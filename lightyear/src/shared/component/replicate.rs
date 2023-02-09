use std::any::TypeId;
use std::collections::HashMap;
use bevy_ecs::component::{Component, ComponentStorage, TableStorage};
use bevy_ecs::entity::Entity;
use bevy_reflect::{FromReflect, Reflect};
use lightyear_serde::{BitReader, BitWrite, Serde, Error};

use crate::shared::messages::named::Named;
use crate::shared::types::ComponentId;
use crate::shared::{component::component_update::ComponentUpdate, NetEntityConverter};


/// A map to hold all component types
pub struct Components {
    pub current_id: u16,
    pub type_to_id_map: HashMap<TypeId, ComponentId>,
}

impl Components {
    pub fn type_to_id<C: Component>(&self) -> ComponentId {
        let type_id = TypeId::of::<C>();
        *self.type_to_id_map.get(&type_id).expect("Must properly initialize Component with Protocol via `add_component()` function!")
    }
    pub fn id_to_name(id: &ComponentId) -> String {
        todo!()
    }

    pub fn read(
        bit_reader: &mut BitReader,
        converter: &dyn NetEntityConverter,
    ) -> Result<Box<dyn ReplicableComponent>, Error> {
        todo!()
    }

    pub fn write(
        bit_writer: &mut dyn BitWrite,
        converter: &dyn NetEntityConverter,
        message: &Box<dyn ReplicableComponent>,
    ) {
        todo!()
    }
}


pub trait ReplicableComponent: Component<Storage=TableStorage> + Replicate {}

/// A struct that implements Replicate is a Message/Component that can be replicated/synced
/// between server and client.
///
/// It may contain [`Entity`] fields which will be serialized into [`NetEntity`]
pub trait Replicate: Serde + Reflect + FromReflect {
    /// Returns whether has any EntityProperties
    fn has_entity_properties(&self) -> bool;

    /// Returns a list of Entities contained within the Replica's properties
    fn entities(&self) -> Vec<Entity>;

    /// Writes data into an outgoing byte stream, sufficient to completely
    /// recreate the Message/Component on the client
    fn write(&self, bit_writer: &mut dyn BitWrite, converter: &dyn NetEntityConverter);

    /// Read data from the incoming byte stream and reconstruct the Message/Component
    fn read(
        bit_reader: &mut BitReader,
        converter: &dyn NetEntityConverter,
    ) -> Result<Self, Error>;
}


cfg_if! {
    if #[cfg(feature = "bevy_support")]
    {
        // Require that Bevy Component to be implemented
        use bevy_ecs::component::{TableStorage, Component};

        pub trait ReplicateInner: Component<Storage = TableStorage> + Sync + Send + 'static {}

        impl<T> ReplicateInner for T
        where T: Component<Storage = TableStorage> + Sync + Send + 'static {
        }
    }
    else
    {
        pub trait ReplicateInner: Sync + Send + 'static {}

        impl<T> ReplicateInner for T
        where T: Sync + Send + 'static {
        }
    }
}
