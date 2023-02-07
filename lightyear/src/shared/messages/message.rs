use crate::shared::messages::named::Named;
use crate::shared::{MessageId, NetEntityConverter};
use lightyear_serde::{BitReader, BitWrite, SerdeErr};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use bevy_ecs::entity::Entity;

/// Messages protocol
pub struct Messages {
    pub current_id: u16,
    pub type_to_id_map: HashMap<TypeId, MessageId>,
}

impl Messages {
    pub fn type_to_id<M: Message>(&self) -> MessageId {
        let type_id = TypeId::of::<M>();
        *self.messages_data.type_to_id_map.get(&type_id).expect(
            "Must properly initialize Message with Protocol via `add_message()` function!",
        )
    }

    pub fn add_message<M: Message>(&mut self) {
        let type_id = TypeId::of::<M>();
        let message_id = MessageId::new(self.current_id);
        self.type_to_id_map.insert(type_id, message_id);
        self.current_id += 1;
    }

    pub fn message_id_from_box(boxed_message: &Box<dyn Message>) -> MessageId {
        todo!()
    }

    pub fn downcast<M: Message>(boxed_message: Box<dyn Message>) -> Option<M> {
        let boxed_any: Box<dyn Any> = boxed_message.into_any();
        Box::<dyn Any + 'static>::downcast::<M>(boxed_any)
            .ok()
            .map(|boxed_m| *boxed_m)
    }

    pub fn read(
        bit_reader: &mut BitReader,
        converter: &dyn NetEntityConverter,
    ) -> Result<Box<dyn Message>, SerdeErr> {
        todo!()
    }

    pub fn write(
        bit_writer: &mut dyn BitWrite,
        converter: &dyn NetEntityConverter,
        message: &Box<dyn Message>,
    ) {
        todo!()
    }
}

// Message
pub trait Message: Send + Sync + Named + MessageClone + Any {
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
    fn has_entity_properties(&self) -> bool;
    /// Returns a list of Entities contained within the Replica's properties
    fn entities(&self) -> Vec<Entity>;
}

// Named
impl Named for Box<dyn Message> {
    fn name(&self) -> String {
        self.as_ref().name()
    }
}

// MessageClone
pub trait MessageClone {
    fn clone_box(&self) -> Box<dyn Message>;
}

impl<T: 'static + Clone + Message> MessageClone for T {
    fn clone_box(&self) -> Box<dyn Message> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn Message> {
    fn clone(&self) -> Box<dyn Message> {
        MessageClone::clone_box(self.as_ref())
    }
}
