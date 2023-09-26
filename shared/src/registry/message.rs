use crate::channel::channel::Channel;
use crate::packet::message::Message;
use crate::registry::TypeRegistry;

pub(crate) struct MessageRegistry(TypeRegistry);

// impl MessageRegistry {
//     fn add(&mut self, message: Message) -> anyhow::Result<()> {
//         self.0.add(message)
//     }
//
//     fn get<M: Message>(&self) -> Option<&M> {
//         self.0.get::<M>()
//     }
// }
