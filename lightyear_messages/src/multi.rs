use crate::registry::MessageRegistry;
use crate::send::Priority;
use crate::{Message, MessageManager};
use bevy::ecs::entity::EntitySet;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use lightyear_serde::entity_map::SendEntityMap;
use lightyear_serde::writer::Writer;
use lightyear_transport::channel::Channel;
use lightyear_transport::prelude::Transport;

#[derive(SystemParam)]
struct MultiMessageSender<'w, 's> {
    players: Query<'w, 's, (&'static mut MessageManager, &'static mut Transport)>,
    registry: Res<'w, MessageRegistry>,
    // TODO: should we let users provide their own Writer?
    writer: Local<'s, Writer>
}

impl<'w, 's> MultiMessageSender<'w, 's> {
    pub fn send_with_priority<M: Message, C: Channel>(
        &mut self,
        message: &M,
        senders: impl EntitySet,
        priority: Priority
    ) -> Result {
        // if the message is not map-entities, we can serialize it once and clone the bytes
        if !self.registry.is_map_entities::<M>()? {
            // TODO: serialize once for all senders. Figure out how to get a shared writer. Maybe on Server? Or as a global resource?
            //   or as Local?
            self.registry.serialize::<M>(
                message,
                &mut self.writer,
                &mut SendEntityMap::default()
            )?;
            let bytes = self.writer.split();
            self.players.iter_many_unique_mut(senders).try_for_each(|(_, mut transport)| {
                transport.send_with_priority::<C>(bytes.clone(), priority)
            })?;
        } else {
            self.players.iter_many_unique_mut(senders).try_for_each(|(mut manager, mut transport)| {
                self.registry.serialize::<M>(
                    message,
                    &mut self.writer,
                    // TODO: ideally we could do entity mapping without Mut!!!
                    &mut manager.entity_mapper.local_to_remote,
                )?;
                let bytes = self.writer.split();
                transport.send_with_priority::<C>(bytes, priority)?;
                Ok::<(), BevyError>(())
            })?;
        }
        Ok::<(), _>(())
    }

    pub fn send<M: Message, C: Channel>(
        &mut self,
        message: &M,
        senders: impl EntitySet,
    ) -> Result {
        self.send_with_priority::<M, C>(message, senders, 1.0)
    }
}