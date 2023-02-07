use crate::shared::messages::named::Named;
use crate::shared::{MessageId, NetEntityConverter};
use lightyear_serde::{BitReader, BitWrite, SerdeErr};
use std::any::Any;
use bevy_ecs::entity::Entity;

// Messages
pub struct Messages {}

impl Messages {
    pub fn type_to_id<M: Message>() -> MessageId {
        todo!()
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
